//! Embeds the Windows resources (the app icon) into the executable.

fn main() {
    println!("cargo:rerun-if-changed=laqta.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/laqta.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // A missing resource compiler only costs the icon, not the build.
        if let Err(e) = embed_resource::compile("laqta.rc", embed_resource::NONE).manifest_optional() {
            println!("cargo:warning=app icon not embedded: {e}");
        }
    }
}
