//! Lightweight in-tree benchmark harness and micro-benchmark suite for codefeed core.
//!
//! Run with:
//!   cargo bench
//!   cargo bench -- --filter trie
//!   cargo bench -- --json

use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use codefeed::__bench::{
    BudgetTrie, CollectionResult, DigestFormat, FileStorage, Item, ItemPath, OutputTarget, Probe,
    Selector, collect_local, estimate_tokens, render_digest_to_writer, text_content,
    write_xml_escaped, xml_escape,
};

pub struct VoidWriter;
impl Write for VoidWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

mod engine {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub struct BenchmarkResult {
        pub name: String,
        pub iterations: usize,
        pub min_ns: u64,
        pub mean_ns: u64,
        pub throughput_mbs: Option<f64>,
    }

    pub struct Runner {
        filter: Option<String>,
        results: Vec<BenchmarkResult>,
    }

    impl Runner {
        pub fn new(filter: Option<String>) -> Self {
            Self {
                filter,
                results: Vec::new(),
            }
        }

        pub fn should_run(&self, name: &str) -> bool {
            match &self.filter {
                Some(pattern) => name.contains(pattern),
                None => true,
            }
        }

        pub fn bench<F: FnMut()>(
            &mut self,
            name: &str,
            warmup: usize,
            iterations: usize,
            throughput_bytes: Option<u64>,
            mut f: F,
        ) {
            if !self.should_run(name) {
                return;
            }

            for _ in 0..warmup {
                f();
            }

            let mut min_ns = u64::MAX;
            let mut total_ns: u128 = 0;

            for _ in 0..iterations {
                let start = Instant::now();
                f();
                let elapsed = start.elapsed().as_nanos() as u64;
                min_ns = min_ns.min(elapsed);
                total_ns += elapsed as u128;
            }

            let effective_iterations = iterations.max(1);
            let mean_ns = (total_ns / effective_iterations as u128) as u64;

            let throughput_mbs = throughput_bytes.map(|bytes| {
                if mean_ns > 0 {
                    (bytes as f64 / 1_000_000.0) / (mean_ns as f64 / 1_000_000_000.0)
                } else {
                    0.0
                }
            });

            self.results.push(BenchmarkResult {
                name: name.to_string(),
                iterations: effective_iterations,
                min_ns,
                mean_ns,
                throughput_mbs,
            });
        }

        pub fn results(&self) -> &[BenchmarkResult] {
            &self.results
        }
    }
}

mod fixture {
    use super::*;

    pub fn generate_flat_files(count: usize, file_size: usize) -> FileStorage {
        let line = "pub fn sample() -> i32 { 42 }\n";
        let repeats = (file_size / line.len()).max(1);
        let content = line.repeat(repeats);

        let mut storage = FileStorage::new();
        for i in 0..count {
            let path = format!("src/file_{i:05}.rs");
            storage.push(&path, &content);
        }
        storage
    }

    pub fn generate_deep_files(count: usize, max_depth: usize, file_size: usize) -> FileStorage {
        let line = "const DEPTH_VAL: usize = 100;\n";
        let repeats = (file_size / line.len()).max(1);
        let content = line.repeat(repeats);

        let mut storage = FileStorage::new();
        for i in 0..count {
            let mut path = String::new();
            for d in 0..max_depth {
                path.push_str(&format!("level_{d}/"));
            }
            path.push_str(&format!("node_{i:05}.rs"));
            storage.push(&path, &content);
        }
        storage
    }

    pub fn generate_xml_heavy_files(count: usize, file_size: usize) -> FileStorage {
        let line = "if (a < b && c > d) { let s = \"'quotes' & <tags>\"; }\n";
        let repeats = (file_size / line.len()).max(1);
        let content = line.repeat(repeats);

        let mut storage = FileStorage::new();
        for i in 0..count {
            let path = format!("src/xml_{i:05}.rs");
            storage.push(&path, &content);
        }
        storage
    }

    pub fn build_collection(label: &str, files: FileStorage) -> CollectionResult {
        let mut trie = BudgetTrie::new();
        for file in files.iter() {
            trie.insert(file.path);
        }
        CollectionResult {
            label: label.to_string(),
            revision: Some("main".to_string()),
            files,
            skipped: 0,
            trie,
        }
    }

    pub struct TempDirGuard(pub std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn setup_temp_hierarchy(file_count: usize) -> (TempDirGuard, std::path::PathBuf) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("codefeed_bench_io_{timestamp}"));
        std::fs::create_dir_all(&path).expect("create temp bench dir");

        let content = b"pub fn sample() -> i32 { 42 }\n";
        let dirs_count = 10.min(file_count);
        let subs_count = 10.min(file_count / dirs_count.max(1)).max(1);
        let files_per_sub = (file_count / (dirs_count * subs_count)).max(1);

        let mut created = 0;
        'outer: for d in 0..dirs_count {
            for s in 0..subs_count {
                let dir_path = path.join(format!("dir_{d:02}")).join(format!("sub_{s:02}"));
                std::fs::create_dir_all(&dir_path).expect("create sub dir");
                for f in 0..files_per_sub {
                    let file_path = dir_path.join(format!("file_{f:02}.rs"));
                    std::fs::write(&file_path, content).expect("write file");
                    created += 1;
                    if created >= file_count {
                        break 'outer;
                    }
                }
            }
        }

        (TempDirGuard(path.clone()), path)
    }
}

mod format {
    use super::engine::BenchmarkResult;

    pub enum OutputFormat {
        Table,
        Json,
    }

    pub fn print_results(results: &[BenchmarkResult], format: OutputFormat) {
        match format {
            OutputFormat::Table => print_table(results),
            OutputFormat::Json => print_json(results),
        }
    }

    fn print_table(results: &[BenchmarkResult]) {
        println!(
            "\n{:<30} {:>12} {:>14} {:>14} {:>18}",
            "Benchmark", "Iterations", "Min (us)", "Mean (us)", "Throughput (MB/s)"
        );
        println!("{}", "-".repeat(92));
        for r in results {
            let min_us = format!("{:.2}", r.min_ns as f64 / 1_000.0);
            let mean_us = format!("{:.2}", r.mean_ns as f64 / 1_000.0);
            let throughput = match r.throughput_mbs {
                Some(mbs) => format!("{mbs:.2}"),
                None => "-".to_string(),
            };
            println!(
                "{:<30} {:>12} {:>14} {:>14} {:>18}",
                r.name, r.iterations, min_us, mean_us, throughput
            );
        }
        println!();
    }

    fn print_json(results: &[BenchmarkResult]) {
        println!("[");
        for (i, r) in results.iter().enumerate() {
            let comma = if i + 1 < results.len() { "," } else { "" };
            let throughput = match r.throughput_mbs {
                Some(val) => format!("{val:.2}"),
                None => "null".to_string(),
            };
            println!(
                "  {{\"name\": \"{}\", \"iterations\": {}, \"min_ns\": {}, \"mean_ns\": {}, \"throughput_mbs\": {}}}{}",
                r.name, r.iterations, r.min_ns, r.mean_ns, throughput, comma
            );
        }
        println!("]");
    }
}

mod suites {
    use super::engine::Runner;
    use super::fixture::*;
    use super::*;

    pub fn run_all(runner: &mut Runner) {
        bench_trie(runner);
        bench_trie_render(runner);
        bench_rendering(runner);
        bench_xml_primitives(runner);
        bench_token_estimation(runner);
        bench_text_content(runner);
        bench_selector_decide(runner);
        bench_local_collect(runner);
    }

    fn bench_trie(runner: &mut Runner) {
        let configs = [
            ("trie_flat_100", 100, 1, 50, 100),
            ("trie_flat_1000", 1_000, 1, 20, 50),
            ("trie_flat_10000", 10_000, 1, 5, 10),
            ("trie_deep_100", 100, 8, 50, 100),
            ("trie_deep_1000", 1_000, 8, 20, 50),
            ("trie_deep_10000", 10_000, 8, 5, 10),
        ];

        for (name, count, depth, warmup, iterations) in configs {
            let paths: Vec<String> = if depth == 1 {
                (0..count).map(|i| format!("src/file_{i:05}.rs")).collect()
            } else {
                (0..count)
                    .map(|i| {
                        let mut p = String::new();
                        for d in 0..depth {
                            p.push_str(&format!("level_{d}/"));
                        }
                        p.push_str(&format!("node_{i:05}.rs"));
                        p
                    })
                    .collect()
            };

            runner.bench(name, warmup, iterations, None, || {
                let mut trie = BudgetTrie::new();
                for p in &paths {
                    trie.insert(p);
                }
                black_box(trie);
            });
        }
    }

    fn bench_trie_render(runner: &mut Runner) {
        let flat_paths: Vec<String> = (0..1_000).map(|i| format!("src/file_{i:05}.rs")).collect();
        let mut flat_trie = BudgetTrie::new();
        for p in &flat_paths {
            flat_trie.insert(p);
        }

        runner.bench("trie_render_flat_1000", 20, 50, None, || {
            let mut writer = VoidWriter;
            flat_trie
                .render_to_writer(&mut writer, false)
                .expect("render trie");
            black_box(&mut writer);
        });

        let deep_paths: Vec<String> = (0..1_000)
            .map(|i| {
                let mut p = String::new();
                for d in 0..8 {
                    p.push_str(&format!("level_{d}/"));
                }
                p.push_str(&format!("node_{i:05}.rs"));
                p
            })
            .collect();
        let mut deep_trie = BudgetTrie::new();
        for p in &deep_paths {
            deep_trie.insert(p);
        }

        runner.bench("trie_render_deep_1000", 20, 50, None, || {
            let mut writer = VoidWriter;
            deep_trie
                .render_to_writer(&mut writer, false)
                .expect("render trie");
            black_box(&mut writer);
        });
    }

    fn bench_rendering(runner: &mut Runner) {
        let cases = [
            ("render_text_1000", DigestFormat::Text, false),
            ("render_xml_1000", DigestFormat::Xml, true),
        ];

        for (name, format, xml_heavy) in cases {
            let files = if xml_heavy {
                generate_xml_heavy_files(1_000, 2_000)
            } else {
                generate_flat_files(1_000, 2_000)
            };
            let approx_bytes: u64 = files
                .iter()
                .map(|f| (f.path.len() + f.content.len()) as u64)
                .sum();
            let collection = build_collection("bench_repo", files);

            runner.bench(name, 5, 20, Some(approx_bytes), || {
                let mut sink = Vec::with_capacity(2 * 1024 * 1024);
                let tokens = render_digest_to_writer(&mut sink, &collection, format)
                    .expect("rendering should succeed");
                black_box(tokens);
                black_box(sink.len());
            });
        }
    }

    fn bench_xml_primitives(runner: &mut Runner) {
        let chunk = "if (a < b && c > d) { let s = \"'quotes' & <tags>\"; }\n";
        let repeats = (100_000 / chunk.len()).max(1);
        let payload = chunk.repeat(repeats);
        let payload_bytes = payload.len() as u64;

        runner.bench("xml_escape_100k", 10, 50, Some(payload_bytes), || {
            let escaped = xml_escape(&payload);
            black_box(escaped);
        });

        runner.bench("xml_stream_write_100k", 10, 50, Some(payload_bytes), || {
            let mut writer = VoidWriter;
            let nl = write_xml_escaped(&mut writer, &payload).expect("write xml");
            black_box(nl);
        });
    }

    fn bench_token_estimation(runner: &mut Runner) {
        let configs = [
            ("token_estimate_ascii_2mb", true, 2 * 1024 * 1024),
            ("token_estimate_mixed_2mb", false, 2 * 1024 * 1024),
        ];

        for (name, ascii_only, size) in configs {
            let chunk = if ascii_only {
                "fn compute_something_fast(value: u64) -> u64 { value * 42 + 10 }\n"
            } else {
                "// 한글 주석과 복합 기호: 『a < b && c > d』, \"quotes\"\n"
            };
            let repeats = size / chunk.len();
            let payload = chunk.repeat(repeats).into_bytes();
            let payload_len = payload.len() as u64;

            let payload_str = std::str::from_utf8(&payload).unwrap();

            runner.bench(name, 5, 30, Some(payload_len), || {
                let tokens = estimate_tokens(payload_str);
                black_box(tokens);
            });
        }
    }

    fn generate_selector_items(count: usize) -> Vec<Item> {
        let mut items = Vec::with_capacity(count);
        let sample_bytes = b"pub fn hello() -> &'static str { \"world\" }\n".to_vec();

        for i in 0..count {
            match i % 5 {
                0 => items.push(Item {
                    path: ItemPath::Utf8(format!("src/file_{i}.rs")),
                    probe: Probe::Bytes(sample_bytes.clone()),
                }),
                1 => items.push(Item {
                    path: ItemPath::Utf8(format!("src/image_{i}.png")),
                    probe: Probe::PreclassifiedBinary,
                }),
                2 => items.push(Item {
                    path: ItemPath::Utf8(format!("src/big_{i}.bin")),
                    probe: Probe::TooLarge,
                }),
                3 => items.push(Item {
                    path: ItemPath::Utf8(format!("secret_{i}.key")),
                    probe: Probe::Missing,
                }),
                _ => items.push(Item {
                    path: ItemPath::Utf8(format!("src/valid_{i}.rs")),
                    probe: Probe::Bytes(sample_bytes.clone()),
                }),
            }
        }
        items
    }

    fn bench_selector_decide(runner: &mut Runner) {
        let name = "selector_decide_10000";
        if !runner.should_run(name) {
            return;
        }

        let warmup = 3;
        let iterations = 10;
        let mut batches: Vec<Vec<Item>> = (0..warmup + iterations)
            .map(|_| generate_selector_items(10_000))
            .collect();

        runner.bench(name, warmup, iterations, None, || {
            let items = batches.pop().expect("batch should be available");
            let mut selector = Selector::new();
            for item in items {
                let _ = selector.decide(item);
            }
            let res = selector.finish("bench".into(), None);
            black_box((res.files.len(), res.skipped));
        });
    }

    fn bench_text_content(runner: &mut Runner) {
        let ascii_chunk = "pub fn process_data(val: u64) -> u64 { val * 42 + 7 }\n";
        let repeats = (100_000 / ascii_chunk.len()).max(1);
        let ascii_bytes = ascii_chunk.repeat(repeats).into_bytes();
        let ascii_len = ascii_bytes.len() as u64;

        runner.bench("text_content_ascii_100k", 10, 50, Some(ascii_len), || {
            let res = text_content(ascii_bytes.clone());
            black_box(res);
        });

        let mixed_chunk = "// 한글 주석과 UTF-8 문자열: 『데이터 검증 테스트』 🚀\n";
        let mixed_repeats = (100_000 / mixed_chunk.len()).max(1);
        let mixed_bytes = mixed_chunk.repeat(mixed_repeats).into_bytes();
        let mixed_len = mixed_bytes.len() as u64;

        runner.bench("text_content_mixed_100k", 10, 50, Some(mixed_len), || {
            let res = text_content(mixed_bytes.clone());
            black_box(res);
        });
    }

    fn bench_local_collect(runner: &mut Runner) {
        let name = "local_collect_hierarchical_1000";
        if !runner.should_run(name) {
            return;
        }

        let (_guard, temp_root) = setup_temp_hierarchy(1_000);

        runner.bench(name, 2, 10, None, || {
            let res = collect_local(&temp_root, &OutputTarget::Stdout)
                .expect("collect_local should succeed");
            black_box(res.files.len());
        });
    }
}

mod cli {
    use super::format::OutputFormat;

    pub struct CliConfig {
        pub format: OutputFormat,
        pub filter: Option<String>,
        pub self_test: bool,
    }

    pub fn parse_args(args: &[String]) -> Option<CliConfig> {
        let mut format = OutputFormat::Table;
        let mut filter: Option<String> = None;
        let mut self_test = false;

        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--json" => format = OutputFormat::Json,
                "--filter" => {
                    idx += 1;
                    if idx < args.len() {
                        filter = Some(args[idx].clone());
                    }
                }
                "--test" | "tests" | "--self-test" => {
                    self_test = true;
                }
                "--help" | "-h" => {
                    println!("Usage: cargo bench [-- --json] [--filter <pattern>]");
                    return None;
                }
                arg if arg.starts_with("--filter=") => {
                    filter = Some(arg.trim_start_matches("--filter=").to_string());
                }
                _ => {}
            }
            idx += 1;
        }

        Some(CliConfig {
            format,
            filter,
            self_test,
        })
    }
}

fn run_self_tests() {
    let mut runner = engine::Runner::new(None);
    let mut count = 0;
    runner.bench("test_add", 2, 5, Some(1_000_000), || {
        count += 1;
    });

    let results = runner.results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "test_add");
    assert_eq!(results[0].iterations, 5);
    assert!(results[0].min_ns <= results[0].mean_ns);
    assert!(results[0].throughput_mbs.is_some());
    assert_eq!(count, 7);

    let flat = fixture::generate_flat_files(10, 50);
    assert_eq!(flat.len(), 10);
    assert!(flat.get_at(0).unwrap().path.starts_with("src/file_"));

    let deep = fixture::generate_deep_files(5, 4, 50);
    assert_eq!(deep.len(), 5);
    assert!(
        deep.get_at(0)
            .unwrap()
            .path
            .starts_with("level_0/level_1/level_2/level_3/")
    );

    let xml = fixture::generate_xml_heavy_files(3, 50);
    assert_eq!(xml.len(), 3);
    assert!(xml.get_at(0).unwrap().content.contains("<tags>"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let config = match cli::parse_args(&args) {
        Some(config) => config,
        None => return,
    };

    if config.self_test {
        run_self_tests();
        return;
    }

    let mut runner = engine::Runner::new(config.filter);
    suites::run_all(&mut runner);
    format::print_results(runner.results(), config.format);
}
