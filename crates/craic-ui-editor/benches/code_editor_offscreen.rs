use craic_ui_editor::code_editor::bench_support::{
    OffscreenDocument, generated_csv, highlight_csv, spellcheck_csv,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

fn source_for_rows(rows: usize) -> String {
    if rows == 488
        && let Ok(path) = std::env::var("CRAIC_EDITOR_BENCH_FILE")
        && let Ok(source) = std::fs::read_to_string(path)
    {
        return source;
    }
    generated_csv(rows)
}

fn analysis(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis");
    for rows in [488, 4_880, 10_000] {
        let source = source_for_rows(rows);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("csv_highlight", rows),
            &source,
            |b, source| {
                b.iter(|| black_box(highlight_csv(black_box(source))));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("spellcheck", rows),
            &source,
            |b, source| {
                b.iter(|| black_box(spellcheck_csv(black_box(source))));
            },
        );
    }
    group.finish();
}

fn layout(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout");
    for rows in [488, 4_880, 10_000] {
        let source = source_for_rows(rows);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_function(BenchmarkId::new("measured_wrap_cold", rows), |b| {
            b.iter_batched(
                || OffscreenDocument::new(source.clone()),
                |mut document| black_box(document.measured_layout(true)),
                criterion::BatchSize::SmallInput,
            );
        });
        let mut document = OffscreenDocument::new(source);
        document.measured_layout(true);
        group.bench_function(BenchmarkId::new("dense_markers", rows), |b| {
            b.iter(|| black_box(document.project_dense_markers()));
        });
    }
    group.finish();
}

fn paint(c: &mut Criterion) {
    let mut group = c.benchmark_group("paint_1280x720");
    for rows in [488, 4_880, 10_000] {
        let mut document = OffscreenDocument::new(source_for_rows(rows));
        document.measured_layout(true);
        let middle = document.visual_line_count() / 2;
        group.bench_function(BenchmarkId::new("plain_top", rows), |b| {
            b.iter(|| document.paint_plain(0));
        });
        group.bench_function(BenchmarkId::new("rainbow_top", rows), |b| {
            b.iter(|| document.paint_highlighted(0));
        });
        group.bench_function(BenchmarkId::new("rainbow_middle", rows), |b| {
            b.iter(|| document.paint_highlighted(middle));
        });
    }
    group.finish();
}

criterion_group!(benches, analysis, layout, paint);
criterion_main!(benches);
