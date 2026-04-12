use std::fs;
use std::path::Path;

// rust-embed resolves `ui/dist/` at compile time. In debug builds it reads from
// disk; in release builds it bakes files into the binary. Either way the folder
// must exist. For fresh clones where the UI hasn't been built yet, create a
// placeholder so `cargo build` succeeds and `chorus run` shows a helpful page.
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
<p>This binary was built without a compiled UI. Run <code>chorus setup</code>
(or <code>bun install &amp;&amp; bun run build</code> in <code>ui/</code>) and rebuild.</p>
"#,
        )
        .expect("write placeholder index.html");
    }
}
