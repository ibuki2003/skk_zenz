use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::Dictionary;
use crate::candidate::{CandidateSource, DictionaryCandidate};

pub(crate) struct UserDictionary {
    connection: Connection,
}

impl UserDictionary {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .with_context(|| format!("user辞書を開けません: {}", path.display()))?;
        connection.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS candidates (
                input       TEXT NOT NULL,
                okuri       TEXT NOT NULL,
                text        TEXT NOT NULL,
                annotation  TEXT,
                use_count   INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (input, okuri, text)
            );
            ",
        )?;
        Ok(Self { connection })
    }

    pub(crate) fn register(
        &self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "
            INSERT INTO candidates (input, okuri, text, annotation)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT (input, okuri, text)
            DO UPDATE SET annotation = excluded.annotation
            ",
            params![input, okuri.unwrap_or(""), text, annotation],
        )?;
        Ok(())
    }

    pub(crate) fn select(&self, candidate: &DictionaryCandidate) -> Result<()> {
        self.connection.execute(
            "
            INSERT INTO candidates (input, okuri, text, annotation, use_count)
            VALUES (?1, ?2, ?3, ?4, 1)
            ON CONFLICT (input, okuri, text)
            DO UPDATE SET
                annotation = COALESCE(candidates.annotation, excluded.annotation),
                use_count = candidates.use_count + 1
            ",
            params![
                candidate.input,
                candidate.okuri.as_deref().unwrap_or(""),
                candidate.text,
                candidate.annotation
            ],
        )?;
        Ok(())
    }

    pub(crate) fn purge(&self, input: &str, okuri: Option<&str>, text: &str) -> Result<bool> {
        Ok(self.connection.execute(
            "DELETE FROM candidates
             WHERE input = ?1 AND okuri = ?2 AND text = ?3",
            params![input, okuri.unwrap_or(""), text],
        )? > 0)
    }

    #[allow(dead_code)]
    pub(crate) fn use_count(
        &self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
    ) -> Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT use_count FROM candidates
                 WHERE input = ?1 AND okuri = ?2 AND text = ?3",
                params![input, okuri.unwrap_or(""), text],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
}

impl Dictionary for UserDictionary {
    fn lookup(&self, input: &str, okuri: Option<&str>) -> Result<Vec<DictionaryCandidate>> {
        let mut statement = self.connection.prepare(
            "
            SELECT text, annotation, use_count
            FROM candidates
            WHERE input = ?1 AND okuri = ?2
            ",
        )?;
        let candidates = statement
            .query_map(params![input, okuri.unwrap_or("")], |row| {
                Ok(DictionaryCandidate {
                    input: input.to_owned(),
                    okuri: okuri.map(str::to_owned),
                    text: row.get(0)?,
                    annotation: row.get(1)?,
                    source: CandidateSource::User,
                    use_count: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_system_candidate_is_copied_and_counted() {
        let dictionary = UserDictionary::open(Path::new(":memory:")).unwrap();
        let candidate = DictionaryCandidate {
            input: "かな".to_owned(),
            okuri: None,
            text: "仮名".to_owned(),
            annotation: None,
            source: CandidateSource::System,
            use_count: 0,
        };
        dictionary.select(&candidate).unwrap();
        dictionary.select(&candidate).unwrap();
        assert_eq!(dictionary.use_count("かな", None, "仮名").unwrap(), Some(2));
    }
}
