/// Maximum attempts at inserting a `{base}-{hex4}` agent slug before
/// giving up. 16⁴ = 65_536 suffix combinations per base, so hitting this
/// cap implies either a pathological number of siblings or a real bug.
pub const MAX_SLUG_ATTEMPTS: u32 = 5;

/// 4-character lowercase hex suffix used to keep agent slugs unique.
/// Callers combine it with the user-facing base name as `{base}-{hex4}`
/// and retry on the rare UNIQUE-constraint collision.
pub fn random_slug_suffix() -> String {
    use rand::Rng;
    let n: u16 = rand::rng().random();
    format!("{n:04x}")
}

/// Derive a slug-safe base from user input. Lowercases ASCII letters,
/// keeps digits, collapses runs of other characters into single dashes,
/// and trims leading/trailing dashes. Returns `None` if the input has
/// no ASCII alphanumerics to slug on.
pub fn slugify_base(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut prev_dash = true;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
