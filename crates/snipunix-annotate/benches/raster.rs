//! Benchmarks for the annotation rasteriser and the export encoders.
//!
//! The editor renders the same annotations twice: once for the screen and
//! once for the export. Both go through the shared rasteriser, so a
//! regression here shows up as a slow editor and slow exports.

use criterion::{Criterion, criterion_group, criterion_main};
use snipunix_annotate::{
    AnnotationKind, Color, DocPoint, DocRect, Document, ExportFormat, Line, Preview, RenderEngine,
    Shape, Stroke, encode,
};
use snipunix_capture::CapturedImage;

/// Builds a base image with a gradient.
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

/// Builds a document with a handful of annotations of every shape.
fn document(width: u32, height: u32) -> Document {
    let mut document = Document::new(gradient(width, height)).unwrap();
    for index in 0..20 {
        let offset = f64::from(index) * 12.0;
        document.create_annotation(AnnotationKind::Stroke(Stroke {
            points: vec![
                DocPoint::new(offset, offset),
                DocPoint::new(offset + 120.0, offset + 40.0),
                DocPoint::new(offset + 60.0, offset + 90.0),
            ],
            color: Color::rgb(220, 30, 30),
            width: 3.0,
            highlighter: false,
            opacity: 1.0,
        }));
        document.create_annotation(AnnotationKind::Line(Line {
            start: DocPoint::new(offset, offset),
            end: DocPoint::new(offset + 200.0, offset + 120.0),
            color: Color::rgb(30, 120, 220),
            width: 2.0,
        }));
        document.create_annotation(AnnotationKind::Rectangle(Shape {
            rect: DocRect::new(offset, offset, 140.0, 90.0),
            color: Color::rgb(20, 180, 60),
            width: 2.0,
        }));
        document.create_annotation(AnnotationKind::Ellipse(Shape {
            rect: DocRect::new(offset + 10.0, offset + 10.0, 80.0, 60.0),
            color: Color::rgb(240, 200, 20),
            width: 2.0,
        }));
    }
    document
}

fn rasterise(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("raster");
    for (label, width, height) in [("1080p", 1920, 1080), ("4k", 3840, 2160)] {
        let document = document(width, height);
        let source = document.base_bounds();
        group.bench_function(label, |bencher| {
            let mut engine = RenderEngine::new();
            bencher.iter(|| {
                engine
                    .render(&document, Preview::None, source, 1.0)
                    .unwrap()
            })
        });
    }
    group.finish();
}

fn export_formats(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("export");
    let document = document(1920, 1080);
    let source = document.base_bounds();
    let mut engine = RenderEngine::new();
    let rendered = engine
        .render(&document, Preview::None, source, 1.0)
        .unwrap();
    for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::WebP] {
        group.bench_function(format.label(), |bencher| {
            bencher.iter(|| encode(&rendered, format, 90).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, rasterise, export_formats);
criterion_main!(benches);
