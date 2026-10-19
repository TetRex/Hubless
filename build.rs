fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/panel.slint", config)
        .expect("failed to compile ui/panel.slint");

    println!("cargo:rerun-if-changed=res/protray.rc");
    println!("cargo:rerun-if-changed=res/protray.exe.manifest");
    embed_resource::compile("res/protray.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed resources");
}
