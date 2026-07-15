//! Integration tests for SkillsLoader YAML frontmatter parsing (PKG-09, D-15).

use bastion::agent::skills::SkillsLoader;
use std::io::Write;

#[test]
fn skills_loader_yaml_frontmatter_name_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("weekly-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let mut f = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
    writeln!(
        f,
        "---\nname: weekly-review\ndescription: Review weekly goals\n---\n\n# Weekly Review\n"
    )
    .unwrap();

    let meta = SkillsLoader::load_all(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].name, "weekly-review");
}

#[test]
fn skills_loader_yaml_frontmatter_description_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let mut f = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
    writeln!(
        f,
        "---\nname: test-skill\ndescription: A test skill for validation\n---\n"
    )
    .unwrap();

    let meta = SkillsLoader::load_all(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(meta[0].description, "A test skill for validation");
}

#[test]
fn agentskills_compat_reference_skill_loads() {
    // D-15: skills/weekly-review/SKILL.md loads without modification
    if !std::path::Path::new("skills/weekly-review/SKILL.md").exists() {
        eprintln!("SKIP: skills/weekly-review/SKILL.md not found");
        return;
    }
    let meta = SkillsLoader::load_all("skills/").unwrap();
    let weekly = meta
        .iter()
        .find(|m| m.name.contains("weekly") || m.name.contains("review"));
    assert!(
        weekly.is_some(),
        "skills/weekly-review should load via load_all"
    );
    assert!(!weekly.unwrap().name.is_empty());
    assert!(!weekly.unwrap().description.is_empty());
}

#[test]
fn skills_loader_scan_directory_returns_all_skills() {
    let dir = tempfile::tempdir().unwrap();
    for skill_name in &["skill-a", "skill-b", "skill-c"] {
        let skill_dir = dir.path().join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let mut f = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        writeln!(f, "---\nname: {}\ndescription: test\n---\n", skill_name).unwrap();
    }
    let meta = SkillsLoader::load_all(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(meta.len(), 3, "Expected 3 skills, got: {}", meta.len());
}
