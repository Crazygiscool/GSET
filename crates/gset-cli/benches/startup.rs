use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::process::Command;

fn startup_version(c: &mut Criterion) {
    let binary = std::env::var("GSET_BINARY").unwrap_or_else(|_| {
        "./target/release/gset".to_string()
    });
    c.bench_function("gset version", |b| {
        b.iter(|| {
            let out = Command::new(&binary).arg("version").output().unwrap();
            black_box(out.status.code())
        })
    });
}

criterion_group!(benches, startup_version);
criterion_main!(benches);
