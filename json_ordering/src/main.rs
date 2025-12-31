use indexmap::IndexMap;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Generate a JSON object with 15 keys and random values
fn generate_json_object(rng: &mut impl Rng) -> BTreeMap<String, Value> {
    let keys = [
        "account_id",
        "timestamp",
        "user_email",
        "transaction_type",
        "amount_cents",
        "currency_code",
        "merchant_name",
        "merchant_category",
        "card_last_four",
        "authorization_code",
        "is_international",
        "processing_fee",
        "status",
        "ip_address",
        "user_agent",
    ];

    let mut obj = BTreeMap::new();

    for key in keys {
        let value: Value = match key {
            "account_id" => Value::String(format!("acc_{:08x}", rng.r#gen::<u32>())),
            "timestamp" => Value::String(format!(
                "2024-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                rng.gen_range(1..=12),
                rng.gen_range(1..=28),
                rng.gen_range(0..24),
                rng.gen_range(0..60),
                rng.gen_range(0..60)
            )),
            "user_email" => Value::String(format!("user{}@example.com", rng.gen_range(1000..9999))),
            "transaction_type" => {
                let types = ["purchase", "refund", "withdrawal", "deposit", "transfer"];
                Value::String(types[rng.gen_range(0..types.len())].to_string())
            }
            "amount_cents" => Value::Number(rng.gen_range(100..100000).into()),
            "currency_code" => {
                let currencies = ["USD", "EUR", "GBP", "JPY", "CAD"];
                Value::String(currencies[rng.gen_range(0..currencies.len())].to_string())
            }
            "merchant_name" => {
                let merchants = [
                    "Amazon",
                    "Walmart",
                    "Target",
                    "Best Buy",
                    "Costco",
                    "Home Depot",
                    "Starbucks",
                ];
                Value::String(merchants[rng.gen_range(0..merchants.len())].to_string())
            }
            "merchant_category" => {
                let categories = ["retail", "grocery", "restaurant", "travel", "entertainment"];
                Value::String(categories[rng.gen_range(0..categories.len())].to_string())
            }
            "card_last_four" => Value::String(format!("{:04}", rng.gen_range(0..10000))),
            "authorization_code" => Value::String(format!("{:06}", rng.gen_range(0..1000000))),
            "is_international" => Value::Bool(rng.gen_bool(0.1)),
            "processing_fee" => Value::Number(rng.gen_range(0..500).into()),
            "status" => {
                let statuses = ["approved", "pending", "declined", "cancelled"];
                Value::String(statuses[rng.gen_range(0..statuses.len())].to_string())
            }
            "ip_address" => Value::String(format!(
                "{}.{}.{}.{}",
                rng.gen_range(1..255),
                rng.gen_range(0..255),
                rng.gen_range(0..255),
                rng.gen_range(1..255)
            )),
            "user_agent" => {
                let agents = [
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120.0.0.0",
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15",
                    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) Mobile/15E148",
                    "Mozilla/5.0 (Linux; Android 14) Chrome/120.0.0.0 Mobile",
                ];
                Value::String(agents[rng.gen_range(0..agents.len())].to_string())
            }
            _ => Value::Null,
        };
        obj.insert(key.to_string(), value);
    }

    obj
}

/// Convert BTreeMap to serde_json Map with sorted keys (alphabetical)
fn to_sorted_json(obj: &BTreeMap<String, Value>) -> Value {
    // BTreeMap is already sorted, just convert to Value
    let map: Map<String, Value> = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Value::Object(map)
}

/// Convert BTreeMap to serde_json Value with randomized key order
fn to_random_json(obj: &BTreeMap<String, Value>, rng: &mut impl Rng) -> Value {
    let mut keys: Vec<_> = obj.keys().cloned().collect();
    keys.shuffle(rng);

    // Use IndexMap to preserve insertion order
    let mut map = IndexMap::new();
    for key in keys {
        map.insert(key.clone(), obj.get(&key).unwrap().clone());
    }

    // Convert IndexMap to serde_json Value
    serde_json::to_value(map).unwrap()
}

/// Write objects to JSONL format and return the bytes
fn write_jsonl(objects: &[Value]) -> Vec<u8> {
    let mut buffer = Vec::new();
    for obj in objects {
        serde_json::to_writer(&mut buffer, obj).unwrap();
        buffer.push(b'\n');
    }
    buffer
}

/// Compress data with zstd at default compression level
fn compress_zstd(data: &[u8]) -> Vec<u8> {
    zstd::encode_all(data.as_ref(), 3).unwrap()
}

/// Run benchmark for a specific number of objects
fn run_benchmark(num_objects: usize, seed: u64) {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    // Generate base objects (deterministic based on seed)
    let base_objects: Vec<BTreeMap<String, Value>> =
        (0..num_objects).map(|_| generate_json_object(&mut rng)).collect();

    // Create sorted version (keys alphabetically sorted)
    let sorted_objects: Vec<Value> = base_objects.iter().map(|obj| to_sorted_json(obj)).collect();

    // Create randomized version (keys in random order, different per object)
    let mut rng2 = rand::rngs::StdRng::seed_from_u64(seed + 1000);
    let random_objects: Vec<Value> = base_objects
        .iter()
        .map(|obj| to_random_json(obj, &mut rng2))
        .collect();

    // Write to JSONL
    let sorted_jsonl = write_jsonl(&sorted_objects);
    let random_jsonl = write_jsonl(&random_objects);

    // Compress with zstd
    let sorted_compressed = compress_zstd(&sorted_jsonl);
    let random_compressed = compress_zstd(&random_jsonl);

    // Calculate compression ratios
    let sorted_ratio = sorted_jsonl.len() as f64 / sorted_compressed.len() as f64;
    let random_ratio = random_jsonl.len() as f64 / random_compressed.len() as f64;

    // Calculate the advantage of sorted over random
    let size_diff_uncompressed =
        random_jsonl.len() as i64 - sorted_jsonl.len() as i64;
    let size_diff_compressed =
        random_compressed.len() as i64 - sorted_compressed.len() as i64;
    let pct_diff_compressed = if sorted_compressed.len() > 0 {
        (size_diff_compressed as f64 / sorted_compressed.len() as f64) * 100.0
    } else {
        0.0
    };

    println!("┌─────────────────────────────────────────────────────────────────────────────┐");
    println!(
        "│ Benchmark: {:>6} objects                                                    │",
        num_objects
    );
    println!("├─────────────────────────────────────────────────────────────────────────────┤");
    println!("│                        │    Sorted Keys    │   Random Keys    │    Diff    │");
    println!("├────────────────────────┼───────────────────┼──────────────────┼────────────┤");
    println!(
        "│ Uncompressed (bytes)   │ {:>15} │ {:>14}  │ {:>+9} │",
        sorted_jsonl.len(),
        random_jsonl.len(),
        size_diff_uncompressed
    );
    println!(
        "│ Compressed (bytes)     │ {:>15} │ {:>14}  │ {:>+9} │",
        sorted_compressed.len(),
        random_compressed.len(),
        size_diff_compressed
    );
    println!(
        "│ Compression ratio      │ {:>15.2}x │ {:>14.2}x │ {:>+9.1}% │",
        sorted_ratio, random_ratio, pct_diff_compressed
    );
    println!("└─────────────────────────────────────────────────────────────────────────────┘");
    println!();
}

fn main() {
    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("        JSON Key Ordering Impact on ZSTD Compression - Benchmark Results       ");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();
    println!("Testing JSONL files with sorted vs randomized key ordering.");
    println!("Each JSON object has 15 keys with realistic transaction data.");
    println!("Compression: zstd level 3 (default)");
    println!();

    let test_sizes = [1, 10, 100, 1000, 10000, 100000];
    let seed = 42u64;

    for &size in &test_sizes {
        run_benchmark(size, seed);
    }

    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("                                   Summary                                      ");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();
    println!("Key findings:");
    println!("  - Sorted keys: Keys appear in alphabetical order in every JSON object");
    println!("  - Random keys: Keys appear in different random order per object");
    println!("  - Positive diff: Random ordering uses MORE bytes than sorted");
    println!("  - Negative diff: Random ordering uses FEWER bytes than sorted");
    println!();
    println!("The compression ratio difference demonstrates how consistent key ordering");
    println!("allows zstd's dictionary-based compression to find more repeated patterns.");
    println!();
}
