use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::commands::{OutputFormat, gitignore, utils};

#[derive(Args, Debug)]
pub struct LintArgs {
    /// Path to the Cargo.toml file
    #[arg(short, long, default_value = "Cargo.toml")]
    pub manifest_path: PathBuf,

    /// Output format (text or json)
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,

    /// Custom exclude patterns (space-separated)
    #[arg(short = 'p', long)]
    pub patterns: Option<String>,

    /// Show a diff of what would change (like `create` would do)
    #[arg(long)]
    pub diff: bool,
}

#[derive(Serialize, Debug)]
struct LintReport {
    /// Whether all recommended artefacts are excluded
    valid: bool,
    /// Artefacts that should be excluded but are not
    missing: Vec<String>,
    /// Artefacts that are excluded
    excluded: Vec<String>,
    /// Artefacts already covered by .gitignore
    gitignored: Vec<String>,
    /// Total number of recommended patterns checked
    total_patterns: usize,
    /// README.md is incorrectly listed in package.exclude (should use package.readme instead)
    #[serde(skip_serializing_if = "Option::is_none")]
    readme_in_exclude: Option<String>,
    /// package.readme is not set but README.md exists
    #[serde(skip_serializing_if = "Option::is_none")]
    missing_readme_field: Option<String>,
}

impl LintArgs {
    pub fn run(&self) -> Result<()> {
        let manifest_path = &self.manifest_path;

        // Read and parse Cargo.toml
        let cargo_toml_content = std::fs::read_to_string(manifest_path)
            .with_context(|| format!("Failed to read {}", manifest_path.display()))?;

        let cargo_toml: toml::Value = toml::from_str(&cargo_toml_content)
            .with_context(|| format!("Failed to parse {}", manifest_path.display()))?;

        // Get current exclude list from Cargo.toml
        let current_excludes = utils::get_current_excludes(&cargo_toml);

        // Read .gitignore entries
        // Use current directory when manifest_path has no parent (e.g., default "Cargo.toml")
        let manifest_dir = manifest_path
            .parent()
            .and_then(|p| {
                if p.as_os_str().is_empty() {
                    None
                } else {
                    Some(p)
                }
            })
            .unwrap_or(Path::new("."));
        let gitignore_entries = gitignore::read_gitignore(manifest_dir);

        // Determine patterns to check (normalize by stripping trailing slashes)
        let mut patterns: Vec<String> = match &self.patterns {
            Some(p) => p.split_whitespace().map(utils::normalize_pattern).collect(),
            None => utils::DEFAULT_EXCLUDE_PATTERNS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };

        // Dynamically discover .toml files (excluding Cargo.toml)
        if self.patterns.is_none() {
            patterns.extend(utils::find_toml_files(manifest_dir));
        }

        // Find patterns that exist in the repo but are not excluded and not gitignored
        // README files belong in package.readme, not package.exclude, so skip them here
        let existing_unexcluded: Vec<String> = patterns
            .iter()
            .filter(|pattern| {
                !utils::is_readme_pattern(pattern)
                    && utils::exists_in_repo_exact(manifest_dir, pattern.as_str())
                    && !utils::is_excluded(pattern, &current_excludes)
                    && !gitignore::is_gitignored(pattern, &gitignore_entries)
            })
            .map(|s| s.to_string())
            .collect();

        // Find patterns that are covered (excluded or gitignored)
        let matched_patterns: Vec<String> = patterns
            .iter()
            .filter(|pattern| {
                utils::is_excluded(pattern, &current_excludes)
                    || gitignore::is_gitignored(pattern, &gitignore_entries)
            })
            .map(|s| s.to_string())
            .collect();

        // Find patterns covered only by .gitignore
        let gitignored_only: Vec<String> = patterns
            .iter()
            .filter(|pattern| {
                !utils::is_excluded(pattern, &current_excludes)
                    && gitignore::is_gitignored(pattern, &gitignore_entries)
            })
            .map(|s| s.to_string())
            .collect();

        // Check README.md specific rules:
        // 1. README.md should NOT be in package.exclude (it should be in package.readme)
        // 2. package.readme should be set if README.md exists
        let readme_in_exclude = current_excludes
            .iter()
            .find(|e| e.eq_ignore_ascii_case("README.md"))
            .cloned()
            .map(|e| format!("README.md should not be in package.exclude (use package.readme = \"{}\" instead)", e));

        let readme_exists = utils::exists_in_repo_exact(manifest_dir, "README.md");
        let package_readme = utils::get_package_readme(&cargo_toml);
        let missing_readme_field = if readme_exists && package_readme.is_none() {
            Some("README.md exists but package.readme is not set in Cargo.toml".to_string())
        } else {
            None
        };

        let valid = existing_unexcluded.is_empty()
            && readme_in_exclude.is_none()
            && missing_readme_field.is_none();

        let report = LintReport {
            valid,
            missing: existing_unexcluded.clone(),
            excluded: matched_patterns,
            gitignored: gitignored_only,
            total_patterns: patterns.len(),
            readme_in_exclude,
            missing_readme_field,
        };

        match self.format {
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(&report)?;
                println!("{}", json);
            }
            OutputFormat::Text => {
                if self.diff {
                    // Diff mode: show what would be added
                    if valid {
                        println!("No changes needed — Cargo.toml exclude list is complete.");
                    } else {
                        if !existing_unexcluded.is_empty() {
                            println!(
                                "--- Cargo.toml exclude list is missing {} artefact(s):",
                                existing_unexcluded.len()
                            );
                            for item in &existing_unexcluded {
                                println!("+ {}", item);
                            }
                        }
                        if let Some(msg) = &report.readme_in_exclude {
                            println!("! {}", msg);
                        }
                        if let Some(msg) = &report.missing_readme_field {
                            println!("! {}", msg);
                        }
                    }
                } else {
                    if valid {
                        println!("✓ Cargo.toml exclude list is complete.");
                    } else {
                        if !existing_unexcluded.is_empty() {
                            println!(
                                "✗ Found {} artefact(s) that should be excluded but are not:",
                                existing_unexcluded.len()
                            );
                            for item in &existing_unexcluded {
                                println!("  - {}", item);
                            }
                        }
                        if let Some(msg) = &report.readme_in_exclude {
                            println!("✗ {}", msg);
                        }
                        if let Some(msg) = &report.missing_readme_field {
                            println!("✗ {}", msg);
                        }
                    }
                }
            }
        }

        if !valid {
            std::process::exit(1);
        }

        Ok(())
    }
}
