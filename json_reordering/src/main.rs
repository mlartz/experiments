use clap::Parser;
use indexmap::IndexMap;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    files: Vec<String>,

    /// File containing a list of input files (one per line)
    #[arg(short = 'f', long = "file-list")]
    file_list: Option<String>,

    /// Number of parallel jobs (default: number of CPU cores)
    #[arg(short = 'j', long = "jobs")]
    jobs: Option<usize>,
}

/// Read file paths from a file list (one path per line)
fn read_file_list(path: &str) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut files = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        // Skip empty lines and comments
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            files.push(trimmed.to_string());
        }
    }

    Ok(files)
}

/// Recursively sort all keys in a JSON value alphabetically.
/// For objects, sorts keys and recurses into nested values.
/// For arrays, recurses into each element.
fn sort_keys_recursive(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // Collect keys and sort them
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();

            // Build new map with sorted keys
            let mut sorted_map = Map::with_capacity(map.len());
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
/// Uses a deterministic seed based on object index for reproducibility.
fn randomize_keys_recursive(value: &Value, rng: &mut impl rand::Rng) -> Value {
    match value {
        Value::Object(map) => {
            // Collect keys and shuffle them
            let mut keys: Vec<&String> = map.keys().collect();
            keys.shuffle(rng);

            // Build new IndexMap with shuffled keys (preserves insertion order)
            let mut shuffled: IndexMap<String, Value> = IndexMap::with_capacity(map.len());
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
            let mut preserved: IndexMap<String, Value> = IndexMap::with_capacity(map.len());
            for (k, v) in map.iter() {
                preserved.insert(k.clone(), preserve_keys_recursive(v));
            }
            serde_json::to_value(preserved).unwrap()
        }
        Value::Array(arr) => Value::Array(arr.iter().map(preserve_keys_recursive).collect()),
        _ => value.clone(),
    }
}

/// Read and decompress a zstd-compressed file
fn read_zstd_file(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::Decoder::new(file)?;
    let mut reader = BufReader::with_capacity(256 * 1024, decoder);
    let mut contents = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut contents)?;
    Ok(contents)
}

/// Parse JSONL data into a vector of JSON values (parallel)
fn parse_jsonl_parallel(data: &[u8]) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    // Split into lines first
    let lines: Vec<&[u8]> = data
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty() && !line.iter().all(|&b| b == b' ' || b == b'\t'))
        .collect();

    // Parse lines in parallel
    let values: Result<Vec<Value>, _> = lines
        .par_iter()
        .map(|line| {
            serde_json::from_slice(line)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        })
        .collect();

    values
}

/// Write JSON values to JSONL format with pre-allocated buffer
fn write_jsonl(values: &[Value]) -> Vec<u8> {
    // Estimate size: average ~500 bytes per object
    let mut buffer = Vec::with_capacity(values.len() * 500);
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

/// Write compressed data to a file with buffered writer
fn write_zstd_file(path: &Path, data: &[u8]) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let compressed = compress_zstd(data);
    let compressed_len = compressed.len() as u64;
    let file = File::create(path)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    writer.write_all(&compressed)?;
    writer.flush()?;
    Ok(compressed_len)
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

/// Result of processing a single file
struct ProcessResult {
    input_path: String,
    object_count: usize,
    input_size: u64,
    original_size: u64,
    sorted_size: u64,
    random_size: u64,
    original_path: std::path::PathBuf,
    sorted_path: std::path::PathBuf,
    random_path: std::path::PathBuf,
}

/// Process a single input file with internal parallelization
fn process_file(input_path: &str) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
    let path = Path::new(input_path);

    // Read and decompress input
    let data = read_zstd_file(path)?;
    let original_compressed_size = std::fs::metadata(path)?.len();

    // Parse JSONL in parallel
    let values = parse_jsonl_parallel(&data)?;
    let object_count = values.len();

    // Generate output paths
    let original_path = generate_output_path(path, "original");
    let sorted_path = generate_output_path(path, "sorted");
    let random_path = generate_output_path(path, "random");

    // Process all three variants in parallel
    let (original_size, (sorted_size, random_size)) = rayon::join(
        || {
            // Original: preserve key ordering (parallel over objects)
            let original_values: Vec<Value> =
                values.par_iter().map(preserve_keys_recursive).collect();
            let original_jsonl = write_jsonl(&original_values);
            write_zstd_file(&original_path, &original_jsonl).unwrap()
        },
        || {
            rayon::join(
                || {
                    // Sorted: sort keys alphabetically (parallel over objects)
                    let sorted_values: Vec<Value> =
                        values.par_iter().map(sort_keys_recursive).collect();
                    let sorted_jsonl = write_jsonl(&sorted_values);
                    write_zstd_file(&sorted_path, &sorted_jsonl).unwrap()
                },
                || {
                    // Random: randomize keys with deterministic per-object seeds
                    // Use parallel iterator with index-based seeding for reproducibility
                    let random_values: Vec<Value> = values
                        .par_iter()
                        .enumerate()
                        .map(|(idx, v)| {
                            // Seed RNG based on index for deterministic parallel results
                            let mut rng = rand::rngs::StdRng::seed_from_u64(42 + idx as u64);
                            randomize_keys_recursive(v, &mut rng)
                        })
                        .collect();
                    let random_jsonl = write_jsonl(&random_values);
                    write_zstd_file(&random_path, &random_jsonl).unwrap()
                },
            )
        },
    );

    Ok(ProcessResult {
        input_path: input_path.to_string(),
        object_count,
        input_size: original_compressed_size,
        original_size,
        sorted_size,
        random_size,
        original_path,
        sorted_path,
        random_path,
    })
}

fn main() {
    let args = Args::parse();

    // Configure thread pool if --jobs specified
    if let Some(jobs) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()
            .expect("Failed to configure thread pool");
    }

    // Collect all input files from command line args and file list
    let mut all_files: Vec<String> = args.files.clone();

    if let Some(ref file_list_path) = args.file_list {
        match read_file_list(file_list_path) {
            Ok(files) => {
                println!("Read {} files from {}", files.len(), file_list_path);
                all_files.extend(files);
            }
            Err(e) => {
                eprintln!("Error reading file list {}: {}", file_list_path, e);
                std::process::exit(1);
            }
        }
    }

    if all_files.is_empty() {
        eprintln!("Error: No input files provided. Use positional arguments or --file-list.");
        std::process::exit(1);
    }

    let num_threads = rayon::current_num_threads();
    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("          JSON Key Reordering - Testing Impact on ZSTD Compression            ");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();
    println!("Processing {} files with {} threads", all_files.len(), num_threads);
    println!();

    let success_count = AtomicUsize::new(0);
    let error_count = AtomicUsize::new(0);

    // Process files in parallel
    let results: Vec<_> = all_files
        .par_iter()
        .map(|file| {
            let result = process_file(file);
            match &result {
                Ok(_) => {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("Error processing {}: {}", file, e);
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            result
        })
        .collect();

    // Print results sequentially for clean output
    println!("───────────────────────────────────────────────────────────────────────────────");
    for result in results.into_iter().flatten() {
        println!("File: {}", result.input_path);
        println!("  Parsed {} JSON objects", result.object_count);
        println!(
            "  Input compressed size:    {:>10} bytes",
            result.input_size
        );
        println!("  Output sizes:");
        println!(
            "    Original ordering:      {:>10} bytes -> {}",
            result.original_size,
            result.original_path.display()
        );
        println!(
            "    Sorted keys:            {:>10} bytes -> {}",
            result.sorted_size,
            result.sorted_path.display()
        );
        println!(
            "    Randomized keys:        {:>10} bytes -> {}",
            result.random_size,
            result.random_path.display()
        );

        let sorted_diff = result.sorted_size as i64 - result.original_size as i64;
        let random_diff = result.random_size as i64 - result.original_size as i64;

        println!("  Size differences vs original:");
        println!(
            "    Sorted:     {:>+10} bytes ({:+.2}%)",
            sorted_diff,
            (sorted_diff as f64 / result.original_size as f64) * 100.0
        );
        println!(
            "    Randomized: {:>+10} bytes ({:+.2}%)",
            random_diff,
            (random_diff as f64 / result.original_size as f64) * 100.0
        );
        println!();
    }

    let final_success = success_count.load(Ordering::Relaxed);
    let final_errors = error_count.load(Ordering::Relaxed);

    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!(
        "  Summary: {} files processed successfully, {} errors",
        final_success, final_errors
    );
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    if final_errors > 0 {
        std::process::exit(1);
    }
}
