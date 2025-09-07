# flac-encoder-rs
High-level Rust Flac encoder that uses libflac.

## Examples
### Planar Buffer To `Vec<u8>`
```rust
let flac_data = flac_encoder::FlacBuilder::from_planar(data, sample_rate)
    .compression_level(5)
    .artist("Jane Doe")
    .year(2025)
    .build()
    .unwrap();
```

### Interleaved Buffer To File
```rust
flac_encoder::FlacBuilder::from_interleaved(data, channels, sample_rate)
    .artist("John Doe")
    .title("My Track")
    .write_file("my-track.flac")
    .unwrap();
```
