use std::error::Error;
use std::fs;
use std::path::PathBuf;

use vergen::EmitBuilder;

fn main() -> Result<(), Box<dyn Error>> {
    // Emit `VERGEN_GIT_DESCRIBE` for use in `version!`/Sentry release tagging.
    EmitBuilder::builder()
        .git_describe(true, false, Some("v*"))
        .emit()?;

    // The frontend is built separately (`trunk build` in `ui/`) and embedded via
    // `include_dir!`. Ensure the directory exists with at least a placeholder so the
    // agent always compiles, even before the UI has been built. A real `trunk build`
    // overwrites this placeholder.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let dist = manifest_dir.join("..").join("ui").join("dist");
    fs::create_dir_all(&dist)?;
    let index = dist.join("index.html");
    if !index.exists() {
        fs::write(
            &index,
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Analytics</title></head>\
             <body><p>The UI has not been built. Run <code>trunk build</code> in <code>ui/</code>.</p></body></html>",
        )?;
    }

    Ok(())
}
