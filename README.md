# flac-encoder-rs
Rust Flac encoder that uses libflac.

```rs
let flac_data = flac_encoder::FlacBuilder::new(data, sample_rate)
    .compression_level(5)
    .artist("Jane Doe")
    .year(2025)
    .build();
```
