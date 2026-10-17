# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 164.90 | 161.40 | 1,131.99 | 188.04 | - | - |
| `candidate_budget_contiguous_10000` | 181.90 | 179.60 | 165.65 | 181.67 | - | - |
| `trie_flat_100` | 7.80 | 7.60 | 16.26 | 7.90 | - | - |
| `trie_flat_1000` | 63.50 | 58.60 | 173.83 | 59.58 | - | - |
| `trie_flat_10000` | 857.60 | 793.50 | 1,685.14 | 800.91 | - | - |
| `trie_deep_100` | 25.50 | 24.50 | 31.96 | 24.96 | - | - |
| `trie_deep_1000` | 243.00 | 230.40 | 340.93 | 234.39 | - | - |
| `trie_deep_10000` | 2,687.10 | 2,556.60 | 3,528.15 | 2,944.51 | - | - |
| `trie_presorted_10000` | 1,160.70 | 1,070.60 | 2,114.24 | 1,088.95 | - | - |
| `trie_render_flat_1000` | 16.10 | 15.90 | 14.33 | 18.12 | - | - |
| `trie_render_deep_1000` | 16.40 | 16.10 | 14.42 | 16.30 | - | - |
| `render_text_1000` | 4,270.60 | 4,364.70 | 5,554.31 | 4,588.34 | 359.54 MB/s | 435.23 MB/s |
| `render_xml_1000` | 16,429.80 | 15,823.50 | 18,049.01 | 16,436.65 | 109.54 MB/s | 120.28 MB/s |
| `xml_escape_100k` | 390.20 | 283.50 | 449.80 | 285.24 | 222.23 MB/s | 350.43 MB/s |
| `xml_stream_write_100k` | 193.20 | 192.70 | 350.82 | 194.61 | 284.93 MB/s | 513.64 MB/s |
| `xml_escape_clean_100k` | 98.10 | 186.70 | 170.28 | 189.60 | 586.96 MB/s | 527.15 MB/s |
| `header_escape_clean_100k` | 112.90 | 202.70 | 169.99 | 210.46 | 587.95 MB/s | 474.89 MB/s |
| `header_escape_dirty_100k` | 215.60 | 170.10 | 170.10 | 171.57 | 385.16 MB/s | 582.81 MB/s |
| `token_estimate_ascii_2mb` | 3,054.70 | 3,044.60 | 4,793.74 | 3,159.72 | 437.47 MB/s | 663.70 MB/s |
| `token_estimate_mixed_2mb` | 3,122.60 | 3,161.40 | 4,501.67 | 3,523.12 | 465.86 MB/s | 644.42 MB/s |
| `text_content_ascii_100k` | 9.00 | 9.00 | 29.12 | 9.07 | 3,432.72 MB/s | 11,017.86 MB/s |
| `text_content_mixed_100k` | 87.00 | 85.70 | 122.58 | 104.37 | 815.59 MB/s | 957.93 MB/s |
| `storage_push_sort_presorted_1000` | 28.40 | 28.10 | 29.90 | 32.39 | - | - |
| `storage_push_sort_reverse_1000` | 29.20 | 28.90 | 31.50 | 31.80 | - | - |
| `selector_decide_10000` | 1,892.50 | 1,979.90 | 2,994.36 | 2,256.14 | - | - |
| `local_walk_hierarchical_1000` | 13,331.00 | 14,070.60 | 15,752.81 | 15,241.69 | - | - |
| `local_collect_hierarchical_1000` | 113,754.80 | 119,563.10 | 142,099.03 | 122,209.60 | - | - |
