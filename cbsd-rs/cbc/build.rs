// Read .git-version from workspace root at compile time.
// Production container builds write this file via the Containerfile.
// Dev builds have no file — defaults to "unknown".

use shadow_rs::ShadowBuilder;

fn main() {
    ShadowBuilder::builder()
        .build_pattern(shadow_rs::BuildPattern::RealTime)
        .build()
        .unwrap();
}
