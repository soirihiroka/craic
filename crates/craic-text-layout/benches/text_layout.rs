use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

fn generated_csv(rows: usize) -> String {
    let mut source = String::from("id,p0_x,p0_y,p1_x,p1_y,p2_x,p2_y,p3_x,p3_y\n");
    for row in 0..rows {
        source.push_str(&format!(
            "0-{row},49.17269,381.607,49.17269,397.5426,89.07602,397.5426,89.07602,381.607\n"
        ));
    }
    source
}

fn layout(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_layout");
    for rows in [488, 4_880, 10_000] {
        let source = generated_csv(rows);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("measured_wrap", rows),
            &source,
            |b, source| {
                b.iter(|| {
                    black_box(craic_text_layout::build_visual_lines(
                        black_box(source),
                        &[],
                        true,
                        1_100.0,
                        8.0,
                        |_| 8.0,
                    ))
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("monospace_wrap", rows),
            &source,
            |b, source| {
                b.iter(|| {
                    black_box(craic_text_layout::build_visual_lines_monospace(
                        black_box(source),
                        &[],
                        true,
                        138.0,
                    ))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, layout);
criterion_main!(benches);
