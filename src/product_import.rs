//! Product-owned `.af` blocks: personas, non-secret config, and skill candidates.

use anyhow::Context;
use bastion_mesh::interop::{AgentFile, PersonaEntry, AF_VERSION};
use serde::Serialize;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut};

pub struct PreparedProductImport {
    staging: Option<PathBuf>,
    personas: Vec<(PathBuf, PathBuf)>,
    config: Option<(PathBuf, PathBuf)>,
    config_backup: Option<PathBuf>,
    candidate: Option<(PathBuf, PathBuf)>,
}

#[derive(Serialize)]
struct SoulFront<'a> {
    name: &'a str,
    description: &'a Option<String>,
    bastion: SoulBastion<'a>,
    skills: &'a [String],
}

#[derive(Serialize)]
struct SoulBastion<'a> {
    privacy_tier: &'a str,
    weight: f32,
}

fn safe_slug(name: &str) -> anyhow::Result<String> {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() || slug == "." || slug == ".." || slug.len() > 80 {
        anyhow::bail!("invalid imported persona name")
    }
    Ok(slug)
}

fn soul(entry: &PersonaEntry) -> anyhow::Result<String> {
    if !matches!(entry.tier.as_str(), "cloud-ok" | "local-only") {
        anyhow::bail!(
            "unsupported persona privacy tier '{}': {}",
            entry.name,
            entry.tier
        )
    }
    let yaml = serde_norway::to_string(&SoulFront {
        name: &entry.name,
        description: &entry.description,
        bastion: SoulBastion {
            privacy_tier: &entry.tier,
            weight: entry.weight,
        },
        skills: &entry.skills,
    })?;
    Ok(format!("---\n{yaml}---\n{}\n", entry.system_prompt))
}

impl PreparedProductImport {
    pub fn prepare(
        af: &AgentFile,
        apply: bool,
        managed: bool,
        config_path: &Path,
    ) -> anyhow::Result<Self> {
        Self::prepare_at(af, apply, managed, config_path, Path::new("."))
    }

    fn prepare_at(
        af: &AgentFile,
        apply: bool,
        managed: bool,
        config_path: &Path,
        product_root: &Path,
    ) -> anyhow::Result<Self> {
        if af.version != AF_VERSION {
            anyhow::bail!("unsupported .af version {}", af.version)
        }
        if managed || !apply {
            return Ok(Self {
                staging: None,
                personas: vec![],
                config: None,
                config_backup: None,
                candidate: None,
            });
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let staging = product_root
            .join(".bastion")
            .join(format!("import-staging-{stamp}"));
        std::fs::create_dir_all(&staging)?;
        let prepared = (|| {
            let mut personas = Vec::new();
            for entry in &af.personas {
                let slug = safe_slug(&entry.name)?;
                let destination = product_root.join("personas").join(&slug);
                if destination.exists() {
                    anyhow::bail!(
                        "persona '{}' already exists; import is non-destructive",
                        slug
                    )
                }
                let source = staging.join("personas").join(&slug);
                std::fs::create_dir_all(&source)?;
                std::fs::write(source.join("SOUL.md"), soul(entry)?)?;
                personas.push((source, destination));
            }

            let raw = std::fs::read_to_string(config_path)
                .with_context(|| format!("read {}", config_path.display()))?;
            let mut config_doc: DocumentMut = raw.parse().context("parse Bastion config")?;
            config_doc["agent"]["default_model"] = value(af.config.agent.default_model.clone());
            config_doc["agent"]["daily_budget_usd"] = value(af.config.agent.daily_budget_usd);
            let staged_config = staging.join("bastion.toml");
            let config_backup = staging.join("bastion.toml.backup");
            std::fs::write(&staged_config, config_doc.to_string())?;
            std::fs::write(&config_backup, raw)?;

            let candidate = if af.skills.is_empty() {
                None
            } else {
                let source = staging.join("skill-candidates.json");
                std::fs::write(&source, serde_json::to_vec_pretty(&af.skills)?)?;
                let destination = product_root
                    .join(".bastion")
                    .join("import-candidates")
                    .join(format!("{stamp}.json"));
                Some((source, destination))
            };

            Ok(Self {
                staging: Some(staging.clone()),
                personas,
                config: Some((staged_config, config_path.to_path_buf())),
                config_backup: Some(config_backup),
                candidate,
            })
        })();
        if prepared.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        prepared
    }

    pub fn commit(mut self) -> anyhow::Result<()> {
        let mut published = Vec::new();
        let mut config_published = false;
        let result = (|| -> anyhow::Result<()> {
            if let Some((source, destination)) = &self.config {
                std::fs::rename(source, destination)?;
                config_published = true;
            }
            for (source, destination) in &self.personas {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(source, destination)?;
                published.push(destination.clone());
            }
            if let Some((source, destination)) = &self.candidate {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(source, destination)?;
                published.push(destination.clone());
            }
            Ok(())
        })();
        if let Err(error) = result {
            for destination in published.iter().rev() {
                if destination.is_dir() {
                    let _ = std::fs::remove_dir_all(destination);
                } else {
                    let _ = std::fs::remove_file(destination);
                }
            }
            if config_published {
                if let (Some(backup), Some((_, destination))) = (&self.config_backup, &self.config)
                {
                    std::fs::rename(backup, destination)
                        .context("restore Bastion config after failed import")?;
                }
            }
            return Err(error);
        }
        if let Some(staging) = self.staging.take() {
            let _ = std::fs::remove_dir_all(staging);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_mesh::interop::{AgentConfigExport, ConfigBlock, SkillEntry, PRODUCER_ID};

    fn agent_file() -> AgentFile {
        AgentFile {
            version: AF_VERSION,
            producer: PRODUCER_ID.into(),
            mode: "standalone".into(),
            exported_at: "2026-07-16T00:00:00Z".into(),
            identity: None,
            config: ConfigBlock {
                agent: AgentConfigExport {
                    default_model: "local/new-model".into(),
                    daily_budget_usd: 4.5,
                },
            },
            memories: vec![],
            personas: vec![PersonaEntry {
                name: "Work Coach".into(),
                description: Some("Keeps work focused".into()),
                system_prompt: "Prioritize the next useful action.".into(),
                tier: "local-only".into(),
                weight: 0.8,
                skills: vec!["planning".into()],
            }],
            goals: vec![],
            skills: vec![SkillEntry {
                name: "planning".into(),
                path: "skills/planning".into(),
            }],
        }
    }

    fn config(root: &Path) -> PathBuf {
        let path = root.join("bastion.toml");
        std::fs::write(
            &path,
            "[agent]\ndefault_model = \"old/model\"\ndaily_budget_usd = 1.0\n",
        )
        .expect("write config");
        path
    }

    #[test]
    fn standalone_import_applies_product_blocks_as_one_transaction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = config(dir.path());
        PreparedProductImport::prepare_at(&agent_file(), true, false, &config_path, dir.path())
            .expect("prepare import")
            .commit()
            .expect("commit import");

        let soul = std::fs::read_to_string(dir.path().join("personas/work-coach/SOUL.md"))
            .expect("imported soul");
        assert!(soul.contains("privacy_tier: local-only"));
        assert!(soul.contains("Prioritize the next useful action."));
        let config = std::fs::read_to_string(&config_path).expect("updated config");
        assert!(config.contains("local/new-model"));
        assert!(config.contains("4.5"));
        let candidates = std::fs::read_dir(dir.path().join(".bastion/import-candidates"))
            .expect("candidate directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("candidate entries");
        assert_eq!(candidates.len(), 1);
        assert!(!dir.path().join("skills/planning").exists());
        assert!(std::fs::read_dir(dir.path().join(".bastion"))
            .expect("bastion state")
            .all(|entry| !entry
                .expect("state entry")
                .file_name()
                .to_string_lossy()
                .starts_with("import-staging-")));
    }

    #[test]
    fn managed_import_never_applies_product_owned_state_locally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = config(dir.path());
        let original = std::fs::read_to_string(&config_path).expect("original config");
        PreparedProductImport::prepare_at(&agent_file(), true, true, &config_path, dir.path())
            .expect("prepare managed import")
            .commit()
            .expect("commit managed import");

        assert_eq!(
            std::fs::read_to_string(&config_path).expect("unchanged config"),
            original
        );
        assert!(!dir.path().join("personas").exists());
        assert!(!dir.path().join(".bastion").exists());
    }

    #[test]
    fn conflicts_and_unknown_versions_leave_no_partial_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = config(dir.path());
        let original = std::fs::read_to_string(&config_path).expect("original config");
        std::fs::create_dir_all(dir.path().join("personas/work-coach")).expect("existing persona");
        let error =
            PreparedProductImport::prepare_at(&agent_file(), true, false, &config_path, dir.path())
                .err()
                .expect("conflict must fail");
        assert!(error.to_string().contains("non-destructive"));
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("unchanged config"),
            original
        );
        assert!(std::fs::read_dir(dir.path().join(".bastion"))
            .expect("bastion state")
            .next()
            .is_none());

        let mut unsupported = agent_file();
        unsupported.version = AF_VERSION + 1;
        assert!(PreparedProductImport::prepare_at(
            &unsupported,
            true,
            false,
            &config_path,
            dir.path(),
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("still unchanged"),
            original
        );
    }
}

impl Drop for PreparedProductImport {
    fn drop(&mut self) {
        if let Some(staging) = &self.staging {
            let _ = std::fs::remove_dir_all(staging);
        }
    }
}
