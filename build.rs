use std::fs;
use std::path::Path;

// The embedded UI assets are expected to come from `ui/dist/`, so the folder
// must exist when the crate is compiled. For fresh clones where the UI hasn't
// been built yet, create a placeholder so `cargo build` succeeds and
// `chorus run` shows a helpful page instead of failing on a missing folder.
fn main() {
    println!("cargo:rerun-if-changed=ui/dist");

    let dist = Path::new("ui/dist");
    if !dist.exists() {
        fs::create_dir_all(dist).expect("create ui/dist");
    }
    let index = dist.join("index.html");
    if !index.exists() {
        fs::write(
            &index,
            r#"<!doctype html><meta charset="utf-8"><title>Chorus</title>
<style>body{font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem;color:#222}</style>
<h1>Chorus UI not built</h1>
<p>This binary was built without a compiled UI. Run
<code>bun install &amp;&amp; bun run build</code> in <code>ui/</code> and rebuild.</p>
"#,
        )
        .expect("write placeholder index.html");
    }
}
