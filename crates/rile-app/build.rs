// Windows: embed the Explorer icon and version info into rile.exe.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = "../../packaging/windows/rile.rc";
    println!("cargo:rerun-if-changed={rc}");
    println!("cargo:rerun-if-changed=assets/Rile.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let mut parts: Vec<u16> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).take(3).collect();
    parts.resize(4, 0);
    let comma = parts.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let str_def = format!("RILE_VERSION_STR=\"{version}\"");
    let comma_def = format!("RILE_VERSION_COMMA={comma}");
    // The resource compiler runs on a preprocessed copy in OUT_DIR, so the
    // icon is passed by absolute path.
    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("Rile.ico");
    let icon_def = format!("RILE_ICON=\"{}\"", icon.display().to_string().replace('\\', "\\\\"));
    embed_resource::compile(rc, embed_resource::ParamsMacros([str_def.as_str(), comma_def.as_str(), icon_def.as_str()]))
        .manifest_optional()
        .unwrap();
}
