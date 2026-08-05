//! Regression test for a real bug found 2026-07-25: `/extension install`
//! copied personas straight into `personas_dir()`, but
//! `PersonaRegistry::load_dir` only ever scans `personas_dir()/personas/`
//! -- the two never agreed, so a freshly installed persona could never
//! actually be found by the registry after "restart to activate," no
//! matter how many times you restarted. Every existing test only checked
//! that the SOUL.md file landed on disk, never that the registry actually
//! loads it afterward -- this test closes that gap for good.

use bastion::agent::extension_command::{handle, HandleOutcome};
use bastion::extension::{ExtensionHost, SqliteExtensionStore};
use bastion_personas::persona::PersonaRegistry;

#[tokio::test]
async fn a_persona_installed_via_extension_install_is_findable_by_persona_registry_load_dir() {
    let install_root = tempfile::TempDir::new().unwrap();
    // Simulates BASTION_PERSONAS_DIR pointing at `install_root` -- the exact
    // env var both `/extension install` and `PersonaRegistry::load_dir`
    // read (indirectly, via `bastion::config::personas_dir()` /
    // `personas_install_dir()`).
    let personas_dir = install_root.path().join("personas");

    let pack_root = tempfile::TempDir::new().unwrap();
    std::fs::write(
        pack_root.path().join("pack.toml"),
        r#"
            id = "acme/regression-pack"
            version = "1.0.0"
            extensions = []
            skills = []
            personas = ["test-persona"]

            [defaults]
            enabled_extensions = []
        "#,
    )
    .unwrap();
    let persona_dir = pack_root.path().join("personas").join("test-persona");
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(
        persona_dir.join("SOUL.md"),
        "---\nname: test-persona\nbastion:\n  privacy_tier: cloud-ok\n  weight: 0.5\nobjectives: [\"be a test fixture\"]\ngoals: [\"exist\"]\nscope: \"testing only\"\n---\nbody\n",
    )
    .unwrap();

    let mut host = ExtensionHost::new();
    let store_file = tempfile::NamedTempFile::new().unwrap();
    let store = SqliteExtensionStore::new(store_file.path().to_str().unwrap());
    store.init_schema().await.unwrap();
    let outcome = handle(
        &mut host,
        &store,
        personas_dir.to_str().unwrap(), // <-- this is personas_install_dir()'s shape: personas_dir()/personas
        "/nonexistent/bastion.toml",
        Some(&format!("install {}", pack_root.path().to_str().unwrap())),
        "alice",
    )
    .await
    .unwrap();
    let HandleOutcome::Done(_) = outcome else {
        panic!("pack has no [personas_selection] -- must install immediately");
    };

    // The real assertion: PersonaRegistry::load_dir(install_root), exactly
    // how main.rs calls it with bastion::config::personas_dir(), must find
    // the persona that was just installed.
    let registry = PersonaRegistry::load_dir(install_root.path().to_str().unwrap())
        .await
        .expect("load_dir should succeed even with one persona present");
    assert!(
        registry.get("test-persona").is_some(),
        "a persona installed via /extension install must be loadable by \
         PersonaRegistry::load_dir(personas_dir()) after activation -- if \
         this fails, personas_install_dir()'s \"personas\" join and \
         PersonaRegistry::load_dir's own \"personas\" join have drifted \
         apart again"
    );
}
