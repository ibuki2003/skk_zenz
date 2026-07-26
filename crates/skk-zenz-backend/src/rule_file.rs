use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(crate) struct RuleFile {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    pub(crate) define: HashMap<String, HashMap<String, Value>>,
}

pub(crate) struct RuleFileLoader {
    search_paths: Vec<PathBuf>,
    category: &'static str,
    description: &'static str,
    loaded: HashSet<(String, String)>,
    loading: HashSet<(String, String)>,
}

impl RuleFileLoader {
    pub(crate) fn new(
        search_paths: &[PathBuf],
        category: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            search_paths: search_paths.to_vec(),
            category,
            description,
            loaded: HashSet::new(),
            loading: HashSet::new(),
        }
    }

    pub(crate) fn load(&mut self, rule: &str, name: &str) -> Result<Vec<RuleFile>> {
        let identity = (rule.to_owned(), name.to_owned());
        if self.loaded.contains(&identity) {
            return Ok(Vec::new());
        }
        if !self.loading.insert(identity.clone()) {
            bail!(
                "{}のincludeが循環しています: {rule}/{name}",
                self.description
            );
        }

        let path = self.locate(rule, name)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("{}を読めません: {}", self.description, path.display()))?;
        let rule_file: RuleFile = serde_json::from_str(&contents)
            .with_context(|| format!("{}を解釈できません: {}", self.description, path.display()))?;

        let mut result = Vec::new();
        for include in &rule_file.include {
            let (parent_rule, parent_name) = include
                .split_once('/')
                .map_or((rule, include.as_str()), |(rule, name)| (rule, name));
            result.extend(self.load(parent_rule, parent_name)?);
        }
        result.push(rule_file);

        self.loading.remove(&identity);
        self.loaded.insert(identity);
        Ok(result)
    }

    fn locate(&self, rule: &str, name: &str) -> Result<PathBuf> {
        self.search_paths
            .iter()
            .map(|root| {
                root.join(rule)
                    .join(self.category)
                    .join(format!("{name}.json"))
            })
            .find(|path| path.is_file())
            .with_context(|| {
                format!(
                    "{}が見つかりません: {rule}/{}/{name}.json (search_paths={:?})",
                    self.description, self.category, self.search_paths
                )
            })
    }
}
