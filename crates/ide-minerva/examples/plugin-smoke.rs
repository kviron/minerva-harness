use ide_minerva::Service;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let package = dir.path().join("package");
    std::fs::create_dir_all(package.join("bin")).map_err(|e| e.to_string())?;
    let executable = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(format!("review-plugin{}", std::env::consts::EXE_SUFFIX));
    let name = format!("minerva-review-example{}", std::env::consts::EXE_SUFFIX);
    std::fs::copy(executable, package.join("bin").join(&name)).map_err(|e| e.to_string())?;
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/minerva/plugins/review-example/plugin.json"))
            .map_err(|e| e.to_string())?;
    manifest["backend"]["entry"] = json!(format!("bin/{name}"));
    std::fs::write(package.join("plugin.json"), manifest.to_string()).map_err(|e| e.to_string())?;
    let service = Service::new(dir.path().join("data"), dir.path().join("config"));
    let id = service.install_plugin(package).await?;
    let runtime = service.runtime().await?;
    runtime.plugins.set_enabled(&id, true).await?;
    let result = runtime.plugins.plugin_call(&id, "review.summary", json!({"files":[{"content":"one\ntwo"}]})).await?;
    assert_eq!(result, json!({"fileCount":1,"lineCount":2,"readOnly":true}));
    runtime.plugins.set_enabled(&id, false).await?;
    assert!(runtime.plugins.plugin_call(&id, "review.summary", json!({"files":[]})).await.is_err());
    service.shutdown().await?;
    println!("Minerva package install / enable / call / disable / shutdown: passed");
    Ok(())
}
