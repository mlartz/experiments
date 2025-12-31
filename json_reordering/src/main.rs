use clap::Parser;
use indexmap::IndexMap;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// CLI tool to test JSON key ordering impact on zstd compression.
///
/// Takes zstd-compressed JSONL files and outputs three variants:
/// - original: same key ordering as input
/// - sorted: keys sorted alphabetically (recursive)
/// - random: keys randomized (recursive)
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input zstd-compressed JSONL files
    #[arg(required = true)]
    files: Vec<String>,
}

/// Recursively sort all keys in a JSON value alphabetically.
/// For objects, sorts keys and recurses into nested values.
/// For arrays, recurses into each element.
fn sort_keys_recursive(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // Collect keys and sort them
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();

            // Build new map with sorted keys
            let mut sorted_map = Map::new();
            for key in keys {
                let sorted_value = sort_keys_recursive(map.get(key).unwrap());
                sorted_map.insert(key.clone(), sorted_value);
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => {
            // Recurse into array elements
            Value::Array(arr.iter().map(sort_keys_recursive).collect())
        }
        // Primitives are returned as-is
        _ => value.clone(),
    }
}

/// Recursively randomize all keys in a JSON value.
/// For objects, shuffles keys and recurses into nested values.
/// For arrays, recurses into each element.
fn randomize_keys_recursive(value: &Value, rng: &mut impl rand::Rng) -> Value {
    match value {
        Value::Object(map) => {
            // Collect keys and shuffle them
            let mut keys: Vec<&String> = map.keys().collect();
            keys.shuffle(rng);

            // Build new IndexMap with shuffled keys (preserves insertion order)
            let mut shuffled: IndexMap<String, Value> = IndexMap::new();
            for key in keys {
                let randomized_value = randomize_keys_recursive(map.get(key).unwrap(), rng);
                shuffled.insert(key.clone(), randomized_value);
            }
            // Convert IndexMap to serde_json Value (preserves order due to preserve_order feature)
            serde_json::to_value(shuffled).unwrap()
        }
        Value::Array(arr) => {
            // Recurse into array elements
            Value::Array(
                arr.iter()
                    .map(|v| randomize_keys_recursive(v, rng))
                    .collect(),
            )
        }
        // Primitives are returned as-is
        _ => value.clone(),
    }
}

/// Recursively preserve original key ordering (for arrays, need to recurse).
/// This ensures arrays with nested objects are properly handled.
fn preserve_keys_recursive(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // Use IndexMap to preserve original insertion order
            let preserved: IndexMap<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), preserve_keys_recursive(v)))
                .collect();
            serde_json::to_value(preserved).unwrap()
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(preserve_keys_recursive).collect())
        }
        _ => value.clone(),
    }
}

/// Read and decompress a zstd-compressed file
fn read_zstd_file(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::Decoder::new(file)?;
    let mut reader = BufReader::new(decoder);
    let mut contents = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut contents)?;
    Ok(contents)
}

/// Parse JSONL data into a vector of JSON values
fn parse_jsonl(data: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let reader = BufReader::new(data);
    let mut values = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed)?;
        values.push(value);
    }

    Ok(values)
}

/// Write JSON values to JSONL format
fn write_jsonl(values: &[Value]) -> Vec<u8> {
    let mut buffer = Vec::new();
    for value in values {
        serde_json::to_writer(&mut buffer, value).unwrap();
        buffer.push(b'\n');
    }
    buffer
}

/// Compress data with zstd at default compression level
fn compress_zstd(data: &[u8]) -> Vec<u8> {
    zstd::encode_all(data.as_ref(), 3).unwrap()
}

/// Write compressed data to a file
fn write_zstd_file(path: &Path, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let compressed = compress_zstd(data);
    let mut file = File::create(path)?;
    file.write_all(&compressed)?;
    Ok(())
}

/// Generate output filename by stripping .zst extension and adding suffix
fn generate_output_path(input_path: &Path, suffix: &str) -> std::path::PathBuf {
    let stem = input_path.file_stem().unwrap().to_str().unwrap();

    // If the stem ends with .jsonl, strip it for the base name
    let base_name = if stem.ends_with(".jsonl") {
        &stem[..stem.len() - 6]
    } else {
        stem
    };

    let parent = input_path.parent().unwrap_or(Path::new("."));
    parent.join(format!("{}_{}.jsonl.zst", base_name, suffix))
}

/// Process a single input file
fn process_file(input_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(input_path);

    println!("Processing: {}", input_path);

    // Read and decompress input
    let data = read_zstd_file(path)?;
    let original_compressed_size = std::fs::metadata(path)?.len();

    // Parse JSONL
    let values = parse_jsonl(&data)?;
    println!("  Parsed {} JSON objects", values.len());

    // Create three variants
    let original_values: Vec<Value> = values.iter().map(preserve_keys_recursive).collect();
    let sorted_values: Vec<Value> = values.iter().map(sort_keys_recursive).collect();

    // Use a seeded RNG for reproducibility
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    let random_values: Vec<Value> = values
        .iter()
        .map(|v| randomize_keys_recursive(v, &mut rng))
        .collect();

    // Convert to JSONL
    let original_jsonl = write_jsonl(&original_values);
    let sorted_jsonl = write_jsonl(&sorted_values);
    let random_jsonl = write_jsonl(&random_values);

    // Generate output paths
    let original_path = generate_output_path(path, "original");
    let sorted_path = generate_output_path(path, "sorted");
    let random_path = generate_output_path(path, "random");

    // Write compressed output files
    write_zstd_file(&original_path, &original_jsonl)?;
    write_zstd_file(&sorted_path, &sorted_jsonl)?;
    write_zstd_file(&random_path, &random_jsonl)?;

    // Report sizes
    let original_size = std::fs::metadata(&original_path)?.len();
    let sorted_size = std::fs::metadata(&sorted_path)?.len();
    let random_size = std::fs::metadata(&random_path)?.len();

    println!("  Input compressed size:    {:>10} bytes", original_compressed_size);
    println!("  Output sizes:");
    println!("    Original ordering:      {:>10} bytes -> {}", original_size, original_path.display());
    println!("    Sorted keys:            {:>10} bytes -> {}", sorted_size, sorted_path.display());
    println!("    Randomized keys:        {:>10} bytes -> {}", random_size, random_path.display());

    // Calculate and show differences
    let sorted_diff = sorted_size as i64 - original_size as i64;
    let random_diff = random_size as i64 - original_size as i64;

    println!("  Size differences vs original:");
    println!("    Sorted:     {:>+10} bytes ({:+.2}%)",
             sorted_diff,
             (sorted_diff as f64 / original_size as f64) * 100.0);
    println!("    Randomized: {:>+10} bytes ({:+.2}%)",
             random_diff,
             (random_diff as f64 / original_size as f64) * 100.0);
    println!();

    Ok(())
}

fn main() {
    let args = Args::parse();

    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("          JSON Key Reordering - Testing Impact on ZSTD Compression            ");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    let mut success_count = 0;
    let mut error_count = 0;

    for file in &args.files {
        match process_file(file) {
            Ok(()) => success_count += 1,
            Err(e) => {
                eprintln!("Error processing {}: {}", file, e);
                error_count += 1;
            }
        }
    }

    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Summary: {} files processed successfully, {} errors", success_count, error_count);
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    if error_count > 0 {
        std::process::exit(1);
    }
}
