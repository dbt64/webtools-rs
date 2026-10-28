fn main() {
    slint_build::compile_with_config(
        "ui/launcher.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles),
    )
    .unwrap();
}
