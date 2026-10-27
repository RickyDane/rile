//! Integration tests that build `plugins/hello` for `wasm32-wasip2` and load it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use rile_plugins::{Action, Context, PluginHost};

/// Builds the example plugin once per test binary. Returns `None` (and prints
/// why) if the wasm32-wasip2 target is not installed.
fn hello_wasm() -> Option<&'static Path> {
    static WASM: OnceLock<Option<PathBuf>> = OnceLock::new();
    WASM.get_or_init(|| build_plugin("../../plugins/hello", "rile_plugin_hello"))
        .as_deref()
}

/// The sandbox probe plugin in `tests/probe`.
fn probe_wasm() -> Option<&'static Path> {
    static WASM: OnceLock<Option<PathBuf>> = OnceLock::new();
    WASM.get_or_init(|| build_plugin("tests/probe", "rile_plugin_probe"))
        .as_deref()
}

fn wasm_target_available() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let Ok(out) = Command::new(rustc).args(["--print", "sysroot"]).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&sysroot)
        .join("lib/rustlib/wasm32-wasip2")
        .exists()
}

/// Builds a standalone plugin project (relative to this crate) for
/// wasm32-wasip2 into a separate target dir and returns the `.wasm` path.
fn build_plugin(rel_dir: &str, artifact: &str) -> Option<PathBuf> {
    if !wasm_target_available() {
        eprintln!("SKIPPED: wasm32-wasip2 target not installed (rustup target add wasm32-wasip2)");
        return None;
    }
    let plugin_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_dir);
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugins");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(&plugin_dir)
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("running cargo for plugin");
    assert!(status.success(), "building {rel_dir} failed");
    let wasm = target_dir.join(format!("wasm32-wasip2/release/{artifact}.wasm"));
    assert!(wasm.exists(), "missing {}", wasm.display());
    Some(wasm)
}

fn host_with_hello() -> Option<PluginHost> {
    let wasm = hello_wasm()?;
    let mut host = PluginHost::new().unwrap();
    host.load_file(wasm).unwrap();
    Some(host)
}

#[test]
fn manifest_and_commands() {
    let Some(mut host) = host_with_hello() else {
        return;
    };
    let plugins = host.plugins();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "rile.hello");
    assert_eq!(plugins[0].name, "Hello");
    assert!(plugins[0].enabled);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let ctx = Context {
        current_dir: Some(dir.path().into()),
        selection: vec![file],
    };
    let commands = host.commands(&ctx);
    let labels: Vec<&str> = commands.iter().map(|(_, c)| c.label.as_str()).collect();
    assert_eq!(labels, ["Copy file names", "Count lines"]);
    assert!(commands.iter().all(|(id, _)| id == "rile.hello"));

    // No selection -> no commands; disabled -> no commands.
    assert!(host.commands(&Context::default()).is_empty());
    host.set_enabled("rile.hello", false);
    assert!(!host.plugins()[0].enabled);
    assert!(host.commands(&ctx).is_empty());
    assert!(host.run_command("rile.hello", "copy-names", &ctx).is_err());
}

#[test]
fn load_dir_loads_plugin() {
    let Some(wasm) = hello_wasm() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(wasm, dir.path().join("hello.wasm")).unwrap();
    std::fs::write(dir.path().join("broken.wasm"), b"\0asm garbage").unwrap();
    let mut host = PluginHost::new().unwrap();
    let errors = host.load_dir(dir.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(host.plugins().len(), 1);
    // Loading the same plugin again is rejected as a duplicate id.
    assert!(host.load_file(&dir.path().join("hello.wasm")).is_err());
}

#[test]
fn copy_file_names_and_count_lines() {
    let Some(mut host) = host_with_hello() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("one.txt");
    let b = dir.path().join("two.md");
    std::fs::write(&a, "1\n2\n3\n").unwrap();
    std::fs::write(&b, "a\nb").unwrap();
    let ctx = Context {
        current_dir: Some(dir.path().into()),
        selection: vec![a, b],
    };

    let actions = host.run_command("rile.hello", "copy-names", &ctx).unwrap();
    assert_eq!(actions, [Action::CopyText("one.txt\ntwo.md".into())]);

    let actions = host.run_command("rile.hello", "count-lines", &ctx).unwrap();
    assert_eq!(actions, [Action::Notify("5 lines".into())]);

    assert!(host.run_command("rile.hello", "nope", &ctx).is_err());
    assert!(
        host.run_command("missing.plugin", "copy-names", &ctx)
            .is_err()
    );
}

fn host_with_probe() -> Option<PluginHost> {
    let wasm = probe_wasm()?;
    let mut host = PluginHost::new().unwrap();
    host.load_file(wasm).unwrap();
    Some(host)
}

fn probe(host: &mut PluginHost, command: &str, ctx: &Context) -> String {
    match host
        .run_command("rile.test.probe", command, ctx)
        .unwrap()
        .as_slice()
    {
        [Action::Notify(msg)] => msg.clone(),
        other => panic!("unexpected actions {other:?}"),
    }
}

#[test]
fn read_text_outside_selection_is_denied() {
    let Some(mut host) = host_with_probe() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let selected = dir.path().join("selected.txt");
    std::fs::write(&selected, "hello").unwrap();
    let ctx = Context {
        current_dir: Some(dir.path().into()),
        selection: vec![selected.clone()],
    };

    let read =
        |host: &mut PluginHost, p: &Path| probe(host, &format!("read:{}", p.display()), &ctx);
    assert_eq!(read(&mut host, &selected), "ok:hello");
    assert!(read(&mut host, &secret).starts_with("err:access denied"));
    // Path tricks resolve to the same canonical file and are still denied.
    let sneaky = dir.path().join("sub/../secret.txt");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    assert!(read(&mut host, &sneaky).starts_with("err:access denied"));
    assert!(read(&mut host, Path::new("/etc/passwd")).starts_with("err:access denied"));
    #[cfg(unix)]
    {
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let ctx_link = Context {
            current_dir: None,
            selection: vec![link.clone()],
        };
        // A selected symlink resolves to its target, which the user selected.
        assert_eq!(
            probe(&mut host, &format!("read:{}", link.display()), &ctx_link),
            "ok:top secret"
        );
        // But a symlink is not a way to reach unselected files.
        assert!(read(&mut host, &link).starts_with("err:access denied"));
    }

    // list-dir: current dir allowed, unrelated dirs denied.
    let listed = probe(&mut host, &format!("list:{}", dir.path().display()), &ctx);
    assert!(
        listed.starts_with("ok:") && listed.contains("secret.txt"),
        "{listed}"
    );
    assert!(probe(&mut host, "list:/", &ctx).starts_with("err:access denied"));

    // No WASI escape hatches: no env vars, no preopened filesystem.
    assert_eq!(probe(&mut host, "env", &ctx), "env:0;fs:false");
}

#[test]
fn runaway_plugin_is_stopped_and_recovers() {
    let Some(mut host) = host_with_probe() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "data").unwrap();
    let ctx = Context {
        current_dir: None,
        selection: vec![file.clone()],
    };

    let started = std::time::Instant::now();
    let err = host
        .run_command("rile.test.probe", "spin", &ctx)
        .unwrap_err();
    assert!(err.to_string().contains("execution budget"), "{err:#}");
    eprintln!("spin stopped after {:?}", started.elapsed());

    let err = host
        .run_command("rile.test.probe", "trap", &ctx)
        .unwrap_err();
    assert!(err.to_string().contains("trapped"), "{err:#}");

    // The instance is re-created after a trap.
    assert_eq!(
        probe(&mut host, &format!("read:{}", file.display()), &ctx),
        "ok:data"
    );
}

#[test]
fn csv_preview() {
    let Some(mut host) = host_with_hello() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("data.CSV");
    let mut content = String::from("name,age,city\nAlice,30,\"Paris, FR\"\nBob,4,Rome\n");
    for i in 0..100 {
        content.push_str(&format!("row{i},{i},x\n"));
    }
    std::fs::write(&csv, content).unwrap();

    let preview = host.preview(&csv).expect("csv preview");
    let lines: Vec<&str> = preview.lines().collect();
    assert_eq!(lines[0], "name  | age | city");
    assert_eq!(lines[1], "------+-----+----------");
    assert_eq!(lines[2], "Alice | 30  | Paris, FR");
    assert_eq!(lines[3], "Bob   | 4   | Rome");
    // Header + separator + 49 data rows (first 50 rows in total).
    assert_eq!(lines.len(), 51);

    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, "hi").unwrap();
    assert_eq!(host.preview(&txt), None);
    assert_eq!(host.preview(&dir.path().join("missing.csv")), None);

    host.set_enabled("rile.hello", false);
    assert_eq!(host.preview(&csv), None);
}

#[test]
fn default_plugin_dir_is_set() {
    assert!(rile_plugins::default_plugin_dir().is_some_and(|d| d.ends_with("plugins")));
}
