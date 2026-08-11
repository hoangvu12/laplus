#[cfg(not(windows))]
#[tokio::test]
async fn claude_generates_a_structured_thread_title_with_the_selected_model() {
    use std::os::unix::fs::PermissionsExt;
    use laplus_server::{
        config::ClaudeSettings,
        provider::{ClaudeInstance, ConfiguredInstance, ProviderIdentity},
        text_generation::{Operation, ResultText, Service},
    };

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("claude");
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > arguments\nprintf '%s' \"$CLAUDE_CONFIG_DIR\" > claude-home\nprintf '%s\\n' '{\"structured_output\":{\"title\":\"  Better   title  \"}}'\n",
    ).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let configured_home = directory.path().join("configured-home");
    let instance = ConfiguredInstance::Claude(ClaudeInstance {
        identity: ProviderIdentity {
            instance_id: "claudeAgent".into(),
            driver: "claude-agent".into(),
        },
        display_name: "Claude".into(),
        settings: ClaudeSettings {
            enabled: true,
            binary_path: binary.display().to_string(),
            home_path: configured_home.display().to_string(),
            launch_args: String::new(),
            custom_models: vec![],
        },
    });
    let generated = Service::new().generate(
        &instance,
        directory.path().to_str().unwrap(),
        Some("claude-haiku-4-5"),
        Operation::ThreadTitle { context: "a long first message".into() },
    ).await.unwrap();

    assert_eq!(generated, ResultText::ThreadTitle("Better title".into()));
    let arguments = std::fs::read_to_string(directory.path().join("arguments")).unwrap();
    assert!(arguments.contains("--json-schema"), "{arguments}");
    assert!(arguments.contains("claude-haiku-4-5"), "{arguments}");
    assert!(arguments.contains("a long first message"), "{arguments}");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("claude-home")).unwrap(),
        configured_home.display().to_string(),
    );
}
