use super::*;

#[test]
#[ignore = "Read-only integration with an already running Everything instance"]
fn running_everything_searches_without_es() {
    let results = search("", "wholefilename:Everything.exe")
        .expect("Running Everything must be queried directly; es.exe is not required");
    assert!(results
        .iter()
        .any(|hit| hit.name.eq_ignore_ascii_case("Everything.exe")));
}

#[test]
fn managed_config_keeps_index_settings_but_disables_self_relaunch() {
    let saved = "[Everything]\r\nrun_as_admin=1\r\nindex_size=1\r\nfolders=custom-folder\r\n";
    let config = engine_config(Some(saved));
    assert!(config.contains("run_as_admin=0\r\n"));
    assert!(!config.contains("run_as_admin=1"));
    assert!(config.contains("index_size=1\r\nfolders=custom-folder"));
}

#[test]
#[ignore = "Launches only a named portable Everything against a repository fixture"]
fn portable_engine_indexes_unicode_fixture_and_cleans_up_its_own_process() {
    let existing = ipc::external();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/file-search-portable-test")
        .join(std::process::id().to_string());
    let files = root.join("fixtures");
    std::fs::create_dir_all(&files).unwrap();
    let fixture = files.join("索引探针.txt");
    std::fs::write(&fixture, "file-search fixture").unwrap();
    let ini = root.join("Everything.ini");
    let folder_ini = files
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', r"\\");
    std::fs::write(&ini, format!("{}auto_include_fixed_volumes=0\r\nauto_include_removable_volumes=0\r\nauto_include_fixed_refs_volumes=0\r\nauto_include_removable_refs_volumes=0\r\nfolders=\"{}\"\r\n", engine_config(None), folder_ini)).unwrap();
    let exe =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/everything/Everything.exe");
    let instance = format!("WebToolsFileSearchTest_{}", std::process::id());
    let engine = OwnedEngine::start(
        &exe,
        instance.clone(),
        &ini,
        &root.join("Everything.db"),
        false,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let hwnd = loop {
        if let Some(hwnd) = ipc::find(Some(&instance)) {
            if ipc::ready(hwnd).unwrap_or(false) {
                break hwnd;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Isolated Everything did not finish the fixture index"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    // Folder indexing is asynchronous even after the base database is loaded.
    let paths = loop {
        let paths = ipc::query(hwnd, "wholefilename:索引探针.txt", &|| true).unwrap();
        if paths
            .iter()
            .any(|path| Path::new(path).canonicalize().ok() == fixture.canonicalize().ok())
        {
            break paths;
        }
        assert!(
            Instant::now() < deadline,
            "Fixture was not indexed: {paths:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    eprintln!("portable Unicode results={paths:?}");
    drop(engine);
    assert!(
        ipc::find(Some(&instance)).is_none(),
        "Owned index engine must close with its owner"
    );
    assert_eq!(
        ipc::external(),
        existing,
        "The shared Everything instance must not be closed"
    );
}

#[test]
#[ignore = "Launches isolated helper/engine processes; no UAC or shared Everything changes"]
fn helper_follows_parent_and_job_owns_the_index_engine() {
    const FILTER: &str = "file_search::tests::helper_follows_parent_and_job_owns_the_index_engine";
    match std::env::var("WT_FS_HELPER_TEST").ok().as_deref() {
        Some("parent") => {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        Some("helper") => {
            let root = PathBuf::from(std::env::var_os("WT_FS_HELPER_ROOT").unwrap());
            helper::supervise(
                std::env::var("WT_FS_HELPER_PARENT")
                    .unwrap()
                    .parse()
                    .unwrap(),
                &Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../resources/everything/Everything.exe"),
                std::env::var("WT_FS_HELPER_INSTANCE").unwrap(),
                &root.join("Everything.ini"),
                &root.join("Everything.db"),
            )
            .unwrap();
            return;
        }
        _ => {}
    }
    struct TestProcess(Child);
    impl Drop for TestProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let original = ipc::external();
    for case in ["parent-exit", "helper-exit"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/file-search-helper-test")
            .join(format!("{}-{case}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Everything.ini"),format!("{}auto_include_fixed_volumes=0\r\nauto_include_removable_volumes=0\r\nauto_include_fixed_refs_volumes=0\r\nauto_include_removable_refs_volumes=0\r\n",engine_config(None))).unwrap();
        let instance = format!("WebToolsHelperTest_{}_{case}", std::process::id());
        let spawn = |mode: &str| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", FILTER, "--nocapture"])
                .env("WT_FS_HELPER_TEST", mode)
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command
        };
        let mut parent = TestProcess(spawn("parent").spawn().unwrap());
        let mut supervisor = TestProcess(
            spawn("helper")
                .env("WT_FS_HELPER_ROOT", &root)
                .env("WT_FS_HELPER_PARENT", parent.0.id().to_string())
                .env("WT_FS_HELPER_INSTANCE", &instance)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while ipc::find(Some(&instance)).is_none() {
            assert!(
                Instant::now() < deadline,
                "Helper did not start its named engine"
            );
            std::thread::sleep(Duration::from_millis(40));
        }
        if case == "parent-exit" {
            parent.0.kill().unwrap();
            parent.0.wait().unwrap();
        } else {
            supervisor.0.kill().unwrap();
            supervisor.0.wait().unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while ipc::find(Some(&instance)).is_some() || supervisor.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "{case}: helper/engine did not exit"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
        eprintln!("{case}: named engine and helper exited");
    }
    assert_eq!(ipc::external(), original);
}
