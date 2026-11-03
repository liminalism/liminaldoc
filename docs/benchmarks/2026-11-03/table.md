### Reader: 700-page typeset book PDF (writer export)

| Measure (ms) | Before | After |
|---|---:|---:|
| open | 4.36 | 4.57 |
| first page, first pixels | 8.64 | 6.61 |
| first page, exact | 19.65 | 17.01 |
| page turn exact p50 | 0.04 | 0.04 |
| page turn exact p99 | 23.76 | 10.41 |
| seek exact p50 | 18.87 | 19.43 |
| seek exact p99 | 28.37 | 24.97 |
| zoom exact max | 33.90 | 30.95 |
| search p50 | 15.43 | 15.36 |
| document switch to exact first page | 36.93 | 23.61 |
| RSS MiB | 507.64 | 503.40 |

### Reader: 1000-page synthetic text PDF

| Measure (ms) | Before | After |
|---|---:|---:|
| open | 8.09 | 3.25 |
| first page, first pixels | 7.70 | 2.59 |
| first page, exact | 11.74 | 5.97 |
| page turn exact p50 | 0.03 | 0.04 |
| page turn exact p99 | 6.70 | 7.47 |
| seek exact p50 | 7.08 | 7.74 |
| seek exact p99 | 12.31 | 11.29 |
| zoom exact max | 18.98 | 14.74 |
| search p50 | 0.24 | 0.29 |
| document switch to exact first page | 10.68 | 10.00 |
| RSS MiB | 431.09 | 425.62 |

### Reader: 200-page vector PDF with a raster page

| Measure (ms) | Before | After |
|---|---:|---:|
| open | 0.71 | 0.84 |
| first page, first pixels | 3.06 | 2.91 |
| first page, exact | 7.63 | 6.14 |
| page turn exact p50 | 0.03 | 0.04 |
| page turn exact p99 | 7.16 | 10.24 |
| seek exact p50 | 6.67 | 7.62 |
| seek exact p99 | 8.75 | 11.03 |
| zoom exact max | 18.38 | 18.23 |
| search p50 | 0.05 | 0.05 |
| document switch to exact first page | 10.12 | 8.34 |
| RSS MiB | 324.74 | 323.49 |

### Reader: 1 MB book Markdown (351 pages)

| Measure (ms) | Before | After |
|---|---:|---:|
| open | 440.21 | 296.86 |
| first page, first pixels | 27.58 | 47.70 |
| first page, exact | 45.76 | 70.86 |
| page turn exact p50 | 0.05 | 0.04 |
| page turn exact p99 | 15.79 | 13.71 |
| seek exact p50 | 11.35 | 11.28 |
| seek exact p99 | 43.61 | 16.49 |
| zoom exact max | 23.93 | 14.43 |
| search p50 | 18.60 | 14.77 |
| document switch to exact first page | 574.06 | 272.27 |
| RSS MiB | 440.72 | 419.91 |

### Reader: 300 KB specification Markdown (236 pages)

| Measure (ms) | Before | After |
|---|---:|---:|
| open | 277.41 | 113.89 |
| first page, first pixels | 27.80 | 51.09 |
| first page, exact | 68.05 | 60.65 |
| page turn exact p50 | 0.04 | 0.04 |
| page turn exact p99 | 4.86 | 7.33 |
| seek exact p50 | 7.94 | 9.42 |
| seek exact p99 | 15.56 | 16.94 |
| zoom exact max | 18.55 | 14.11 |
| search p50 | 8.17 | 4.55 |
| document switch to exact first page | 272.23 | 178.66 |
| RSS MiB | 364.69 | 357.75 |

### Writer: 591-page manuscript, 1100×800 frame

| Measure (ms) | Before | After |
|---|---:|---:|
| open bundle | 24.15 | 23.85 |
| first layout + paint | 322.09 | 259.12 |
| type p50 | 9.65 | 3.17 |
| type p95 | 14.03 | 4.73 |
| type p99 | 16.40 | 5.40 |
|   layout inputs (mean) | – | 1.07 |
|   incremental apply (mean) | – | 0.62 |
|   page paint (mean) | – | 1.38 |
|   chrome paint (mean) | – | 0.25 |
| backspace p50 | 10.73 | 3.18 |
| undo p50 | 10.58 | 3.10 |
| page turn p50 | 2.86 | 1.21 |
| go to page p50 | 2.62 | 1.27 |
| find p50 | 14.57 | 6.69 |
| save as | 87.23 | 93.97 |
| reopen + paint | 278.10 | 311.58 |
| RSS MiB | 131.90 | 131.45 |

