/// SkillsLoader loads skills from filesystem (SKILL.md + Rust trait impls).
///
/// `load_all` scans a directory for SKILL.md files and parses their YAML frontmatter (Phase 4).
/// `rescan` parses a single SKILL.md on demand, called by AgentLoop after a `skill_reloaded`
/// signal from skill-writer (Phase 3, D-06).
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
}

/// SKILL.md YAML frontmatter schema (agentskills.io compatible).
#[derive(serde::Deserialize, Default)]
struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    #[allow(dead_code)]
    pub version: Option<String>,
    #[allow(dead_code)]
    pub triggers: Option<Vec<String>>,
}

pub struct SkillsLoader;

impl SkillsLoader {
    /// Scan `skills_dir` for SKILL.md files and parse their YAML frontmatter.
    ///
    /// Returns one SkillMetadata per SKILL.md found. Non-fatal errors (bad frontmatter,
    /// missing files) are logged as warnings; the scan continues.
    ///
    /// YAML frontmatter format (agentskills.io compatible):
    ///   ---
    ///   name: my-skill
    ///   description: "What it does"
    ///   ---
    ///   (markdown body)
    pub fn load_all(skills_dir: &str) -> anyhow::Result<Vec<SkillMetadata>> {
        let base = std::path::Path::new(skills_dir);
        if !base.exists() {
            tracing::warn!(event = "skills_dir_not_found", path = %skills_dir);
            return Ok(vec![]);
        }

        let mut result = Vec::new();

        for entry in std::fs::read_dir(base)
            .map_err(|e| anyhow::anyhow!("failed to read skills dir {}: {}", skills_dir, e))?
        {
            let entry = entry?;
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }

            let skill_md = skill_dir.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            match Self::load_yaml_frontmatter(&skill_md) {
                Ok(meta) => result.push(meta),
                Err(e) => {
                    tracing::warn!(
                        event = "skill_load_error",
                        path = %skill_md.display(),
                        error = %e,
                    );
                }
            }
        }

        tracing::info!(event = "skills_loaded", count = result.len(), dir = %skills_dir);
        Ok(result)
    }

    /// Parse YAML frontmatter from a SKILL.md file.
    fn load_yaml_frontmatter(skill_md: &std::path::Path) -> anyhow::Result<SkillMetadata> {
        let content = std::fs::read_to_string(skill_md)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {}", skill_md.display(), e))?;

        // Extract YAML between first --- and second ---
        let fm = Self::extract_frontmatter(&content).unwrap_or_default();

        // Parse YAML frontmatter — bad frontmatter falls back to defaults (T-04-05-02)
        let parsed: SkillFrontmatter = serde_norway::from_str(&fm).unwrap_or_default();

        // Fall back to directory name if name missing or empty
        let name = parsed.name.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            skill_md
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });

        // description may be a YAML block scalar (>) — serde_norway handles that natively
        let description = parsed
            .description
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();

        Ok(SkillMetadata { name, description })
    }

    /// Extract YAML frontmatter string from content with --- delimiters.
    fn extract_frontmatter(content: &str) -> Option<String> {
        let stripped = content.trim_start();
        if !stripped.starts_with("---") {
            return None;
        }
        // Skip opening ---
        let rest = stripped[3..]
            .trim_start_matches('\n')
            .trim_start_matches('\r');
        let end = rest.find("\n---")?;
        Some(rest[..end].to_owned())
    }

    /// Parse a single SKILL.md at `skill_path` and return its metadata.
    ///
    /// Called by AgentLoop after a `skill_reloaded` signal from skill-writer (D-06).
    /// Extracts `<name>` and `<description>` XML-like tags. If `<name>` is absent,
    /// falls back to the parent directory name (the skill directory name convention).
    pub(crate) fn rescan(skill_path: &str) -> anyhow::Result<SkillMetadata> {
        let content = std::fs::read_to_string(std::path::Path::new(skill_path))
            .map_err(|e| anyhow::anyhow!("skills rescan: cannot read {}: {}", skill_path, e))?;

        let name = Self::extract_tag(&content, "name").unwrap_or_else(|| {
            std::path::Path::new(skill_path)
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });

        let description = Self::extract_tag(&content, "description").unwrap_or_default();

        Ok(SkillMetadata { name, description })
    }

    fn extract_tag(content: &str, tag: &str) -> Option<String> {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        let start = content.find(&open)? + open.len();
        let end = content[start..].find(&close)?;
        Some(content[start..start + end].trim().to_owned())
    }
}

/// A2 `ToolResultObserver` implementation (M2 step 3b): the loop's old
/// `handle_skill_reload` helper, moved here VERBATIM — including ALL of the
/// SEC/CR-02 path sanitization, which protects the rescan and therefore
/// belongs with the implementation, not the kernel.
///
/// D-06: handles the `skill_reloaded` signal emitted by the skill-writer
/// container after a skill is created/updated by natural language.
///
/// Gap 1 fix: this was previously inline in `run_provider_fallback` only,
/// which is unreachable on normal persona turns — so skill-writer-by-NL never
/// reloaded in normal conversation. The kernel consults the observer on BOTH
/// `run_provider_fallback` and `dispatch_tool_loop`, so the skill becomes
/// available on the very next turn regardless of which path produced it.
///
/// Synchronous (no awaits): `SkillsLoader::rescan` and the path checks are sync.
pub struct SkillReloadObserver;

impl bastion_runtime::agent::ports::ToolResultObserver for SkillReloadObserver {
    fn on_tool_result(&self, result: &serde_json::Value) {
        // CR-02 path-safety: rebase skill_path to core's own SKILLS_DIR —
        // skill-writer returns /skills/<name>/SKILL.md (its container path).
        if result.get("skill_reloaded").and_then(|v| v.as_bool()) == Some(true) {
            if let Some(raw_path) = result.get("skill_path").and_then(|v| v.as_str()) {
                let skills_dir =
                    std::env::var("SKILLS_DIR").unwrap_or_else(|_| "/skills".to_string());
                // SEC: skill_path crosses the skill-writer→core container trust
                // boundary. Keep ONLY Normal components — discarding RootDir,
                // Prefix, CurDir and ParentDir ("..") — so a malicious segment
                // cannot escape SKILLS_DIR.
                let normals: Vec<std::path::PathBuf> = std::path::Path::new(raw_path)
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => Some(std::path::PathBuf::from(s)),
                        _ => None,
                    })
                    .collect();
                let skills_base = std::path::Path::new(&skills_dir);
                // Strip the shared skills-base prefix and keep the FULL relative
                // remainder (e.g. "personas/<slug>/<name>/SKILL.md" for private
                // skills). Taking only the last two components would drop the
                // personas/<slug>/ segment and rescan the wrong slot (WR-01).
                let base_norm_count = skills_base
                    .components()
                    .filter(|c| matches!(c, std::path::Component::Normal(_)))
                    .count();
                let tail_components: Vec<std::path::PathBuf> = if normals.len() > base_norm_count {
                    normals[base_norm_count..].to_vec()
                } else {
                    normals.clone()
                };
                // Require the reload target to be <name>/SKILL.md (at least two
                // components, ending in SKILL.md) — guards the format coupling.
                let last_is_skill_md =
                    tail_components.last().and_then(|p| p.to_str()) == Some("SKILL.md");
                if tail_components.len() < 2 || !last_is_skill_md {
                    tracing::warn!(
                        event = "skill_reload_rejected",
                        raw_path = %raw_path,
                        reason = "path does not resolve to <name>/SKILL.md under SKILLS_DIR"
                    );
                } else {
                    let tail: std::path::PathBuf = tail_components.iter().collect();
                    let local_path = skills_base.join(&tail);
                    // Defense in depth: Normal-only components cannot escape
                    // skills_base lexically, but a symlink planted inside
                    // SKILLS_DIR could still redirect rescan outside it. Resolve
                    // symlinks before the containment check. A not-yet-existing
                    // path can't be canonicalized — fall back to the lexical
                    // check; rescan then fails closed on the missing file.
                    let canon_base = std::fs::canonicalize(skills_base)
                        .unwrap_or_else(|_| skills_base.to_path_buf());
                    let contained = match std::fs::canonicalize(&local_path) {
                        Ok(canon) => canon.starts_with(&canon_base),
                        Err(_) => local_path.starts_with(skills_base),
                    };
                    if !contained {
                        tracing::warn!(
                            event = "skill_reload_rejected",
                            path = %local_path.to_string_lossy(),
                            reason = "resolved path escapes SKILLS_DIR"
                        );
                    } else {
                        let path_str = local_path.to_string_lossy();
                        tracing::info!(event = "skill_reload_signal", path = %path_str);
                        match SkillsLoader::rescan(&path_str) {
                            Ok(meta) => tracing::info!(
                                event = "skill_loaded",
                                name = %meta.name,
                                path = %path_str
                            ),
                            Err(e) => tracing::warn!(
                                event = "skill_reload_failed",
                                path = %path_str,
                                err = %e
                            ),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rescan_valid_skill_md_returns_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "<name>weekly-review</name>").unwrap();
        writeln!(f, "<description>Runs a weekly review session</description>").unwrap();

        let meta = SkillsLoader::rescan(path.to_str().unwrap()).unwrap();
        assert_eq!(meta.name, "weekly-review");
        assert_eq!(meta.description, "Runs a weekly review session");
    }

    #[test]
    fn rescan_missing_file_returns_err() {
        let result = SkillsLoader::rescan("/tmp/nonexistent-skill-xyz/SKILL.md");
        assert!(result.is_err(), "should error on missing file");
    }

    #[test]
    fn rescan_skill_md_missing_name_tag_falls_back_to_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "<description>some desc</description>").unwrap();

        let meta = SkillsLoader::rescan(path.to_str().unwrap()).unwrap();
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.description, "some desc");
    }

    #[test]
    fn rescan_extracts_multiline_description() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "<name>test-skill</name>").unwrap();
        writeln!(f, "<description>").unwrap();
        writeln!(f, "  Line one.").unwrap();
        writeln!(f, "  Line two.").unwrap();
        writeln!(f, "</description>").unwrap();

        let meta = SkillsLoader::rescan(path.to_str().unwrap()).unwrap();
        assert_eq!(meta.name, "test-skill");
        assert!(
            meta.description.contains("Line one."),
            "desc: {}",
            meta.description
        );
    }
}
