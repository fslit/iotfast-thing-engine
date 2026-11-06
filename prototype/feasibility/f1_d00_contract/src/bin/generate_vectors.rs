use f1_d00_contract::build_golden_vector_suite;
use std::fs;
use std::path::PathBuf;

fn main() {
    let suite = build_golden_vector_suite();
    let json = serde_json::to_string_pretty(&suite).expect("failed to serialize golden vectors");
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_vectors.json");
    fs::write(&out_path, format!("{}\n", json)).expect("failed to write test_vectors.json");
    println!(
        "Generated {} golden vectors at {}",
        suite.vectors.len(),
        out_path.display()
    );
}
