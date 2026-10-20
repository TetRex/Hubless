fn main() {
    // Переводы интерфейса вшиваются в exe: translations/<язык>/LC_MESSAGES/hubless.po,
    // исходные строки `@tr(...)` — английские, без контекста.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/panel.slint", config)
        .expect("failed to compile ui/panel.slint");
    println!("cargo:rerun-if-changed=translations");

    println!("cargo:rerun-if-changed=res/hubless.rc");
    println!("cargo:rerun-if-changed=res/hubless.exe.manifest");
    embed_resource::compile("res/hubless.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed resources");
}
