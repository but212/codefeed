# Internal Benchmark Results

Execution results from in-tree benchmarks (`benches/bench.rs`).

Environment: Windows x86_64, release profile (`cargo bench`).

| Benchmark | Baseline Min (µs) | Optimized Min (µs) | Baseline Mean (µs) | Optimized Mean (µs) | Baseline Throughput | Optimized Throughput |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `candidate_budget_interleaved_10000` | 1,001.80 | 149.60 | 1,131.99 | 154.38 | - | - |
| `candidate_budget_contiguous_10000` | 165.00 | 175.30 | 165.65 | 181.64 | - | - |
| `trie_flat_100` | 15.10 | 7.90 | 16.26 | 9.22 | - | - |
| `trie_flat_1000` | 169.10 | 62.00 | 173.83 | 67.08 | - | - |
| `trie_flat_10000` | 1,555.20 | 832.20 | 1,685.14 | 845.77 | - | - |
| `trie_deep_100` | 30.20 | 26.40 | 31.96 | 27.56 | - | - |
| `trie_deep_1000` | 335.00 | 248.60 | 340.93 | 260.84 | - | - |
| `trie_deep_10000` | 3,400.00 | 2,697.90 | 3,528.15 | 2,769.77 | - | - |
| `trie_presorted_10000` | 2,010.60 | 1,107.40 | 2,114.24 | 1,135.55 | - | - |
| `trie_render_flat_1000` | 14.00 | 17.90 | 14.33 | 17.96 | - | - |
| `trie_render_deep_1000` | 14.30 | 18.20 | 14.42 | 18.60 | - | - |
| `render_text_1000` | 5,400.50 | 7,028.70 | 5,554.31 | 7,277.34 | 359.54 MB/s | 274.41 MB/s |
| `render_xml_1000` | 17,694.00 | 21,184.10 | 18,049.01 | 21,839.56 | 109.54 MB/s | 90.52 MB/s |
| `xml_escape_100k` | 436.30 | 436.80 | 449.80 | 447.19 | 222.23 MB/s | 223.53 MB/s |
| `xml_stream_write_100k` | 223.10 | 265.10 | 350.82 | 279.15 | 284.93 MB/s | 358.09 MB/s |
| `xml_escape_clean_100k` | 164.00 | 220.60 | 170.28 | 229.17 | 586.96 MB/s | 436.13 MB/s |
| `header_escape_clean_100k` | 152.20 | 204.90 | 169.99 | 217.47 | 587.95 MB/s | 459.59 MB/s |
| `header_escape_dirty_100k` | 251.30 | 218.30 | 259.61 | 223.73 | 385.16 MB/s | 446.93 MB/s |
| `token_estimate_ascii_2mb` | 4,253.90 | 4,134.60 | 4,793.74 | 4,264.13 | 437.47 MB/s | 491.80 MB/s |
| `token_estimate_mixed_2mb` | 4,248.80 | 4,170.00 | 4,501.67 | 4,540.96 | 465.86 MB/s | 461.83 MB/s |
| `text_content_ascii_100k` | 11.30 | 9.00 | 29.12 | 9.04 | 3,432.72 MB/s | 11,054.41 MB/s |
| `text_content_mixed_100k` | 84.60 | 84.20 | 122.58 | 111.76 | 815.59 MB/s | 894.52 MB/s |
| `storage_push_sort_presorted_1000` | 29.20 | 29.40 | 29.90 | 35.43 | - | - |
| `storage_push_sort_reverse_1000` | 29.30 | 29.30 | 31.50 | 30.71 | - | - |
| `selector_decide_10000` | 2,726.80 | 1,904.00 | 2,994.36 | 2,004.32 | - | - |
| `local_collect_hierarchical_1000` | 139,675.50 | 134,447.00 | 142,099.03 | 137,565.04 | - | - |
