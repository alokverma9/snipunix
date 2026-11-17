//! Benchmarks for the capture image pipeline.
//!
//! These cover the hot paths of a capture: decoding the image the portal
//! returned, cropping it to the selection, and encoding the result. The
//! roadmap's performance budgets are expressed in terms of these operations,
//! so a regression here is a regression in the product.

use criterion::{Criterion, criterion_group, criterion_main};
use snipunix_capture::{CapturedImage, decode_image};
use snipunix_core::ImageRect;

/// Builds a deterministic test image with a gradient, so the encoder has
/// something realistic to work with.
fn gradient(width: u32, height: u32) -> CapturedImage {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.push((x % 256) as u8);
            rgba.push((y % 256) as u8);
            rgba.push(((x + y) % 256) as u8);
            rgba.push(255);
        }
    }
    CapturedImage::new(width, height, rgba, Some("benchmark".into())).unwrap()
}

/// Encodes the image once so the decode benchmark measures decoding only.
fn encoded(width: u32, height: u32) -> Vec<u8> {
    gradient(width, height).encode_png().unwrap()
}

fn decode(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("decode");
    for (label, width, height) in [("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let bytes = encoded(width, height);
        group.bench_function(label, |bencher| {
            bencher.iter(|| decode_image(&bytes, Some("benchmark".into())).unwrap())
        });
    }
    group.finish();
}

fn encode(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("encode-png");
    for (label, width, height) in [("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let image = gradient(width, height);
        group.bench_function(label, |bencher| {
            bencher.iter(|| image.encode_png().unwrap())
        });
    }
    group.finish();
}

fn crop(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crop");
    for (label, width, height) in [("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let image = gradient(width, height);
        let rect = ImageRect::new(100, 100, width / 2, height / 2);
        group.bench_function(label, |bencher| bencher.iter(|| image.crop(rect).unwrap()));
    }
    group.finish();
}

criterion_group!(benches, decode, encode, crop);
criterion_main!(benches);
