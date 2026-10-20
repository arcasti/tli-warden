//! Builds the icon into warden.exe, for Explorer, the Start menu and Apps & features.

fn main() {
    println!("cargo:rerun-if-changed=assets/warden.rc");
    println!("cargo:rerun-if-changed=assets/warden.ico");
    embed_resource::compile_for("assets/warden.rc", ["warden"], embed_resource::NONE)
        .manifest_required()
        .expect("assets/warden.rc: the icon resource did not compile");
}
