use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::candidate::{CandidateSource, DictionaryCandidate};

/// Defines the mutable user-dictionary behavior required by conversion.
///
/// Implementations provide learned candidates together with their decayed usage
/// frequency and persist usage updates in a storage-specific way.
pub trait UserDictionary {
    /// Changes the half-life used when calculating effective frequencies.
    fn set_half_life_days(&mut self, half_life_days: f64);

    /// Returns user-dictionary candidates as they would appear at `now`.
    fn lookup_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        now: i64,
    ) -> Result<Vec<DictionaryCandidate>>;

    /// Registers a user-supplied candidate and counts the registration as one use.
    fn register_at(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
        now: i64,
    ) -> Result<()>;

    /// Records one use of a candidate at `now`, updating its decayed frequency.
    fn record_usage(&mut self, candidate: &DictionaryCandidate, now: i64) -> Result<()>;

    /// Removes a candidate from the user dictionary.
    fn purge(&mut self, input: &str, okuri: Option<&str>, text: &str) -> Result<bool>;
}

/// Stores user-dictionary entries and their usage state in SQLite.
pub(crate) struct SqliteUserDictionary {
    connection: Connection,
    half_life_seconds: f64,
}

impl SqliteUserDictionary {
    /// Opens a SQLite user dictionary and creates its schema when necessary.
    pub(crate) fn open(path: &Path, half_life_days: f64) -> Result<Self> {
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
                frequency   REAL NOT NULL DEFAULT 0,
                updated_at  INTEGER NOT NULL,
                PRIMARY KEY (input, okuri, text)
            );
            ",
        )?;
        Ok(Self {
            connection,
            half_life_seconds: half_life_days * 86_400.0,
        })
    }
}

impl UserDictionary for SqliteUserDictionary {
    fn set_half_life_days(&mut self, half_life_days: f64) {
        self.half_life_seconds = half_life_days * 86_400.0;
    }

    fn lookup_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        now: i64,
    ) -> Result<Vec<DictionaryCandidate>> {
        let mut statement = self.connection.prepare(
            "
            SELECT text, annotation, frequency, updated_at
            FROM candidates
            WHERE input = ?1 AND okuri = ?2
            ",
        )?;
        let candidates = statement
            .query_map(params![input, okuri.unwrap_or("")], |row| {
                let frequency = row.get(2)?;
                let updated_at = row.get(3)?;
                Ok(DictionaryCandidate {
                    input: input.to_owned(),
                    okuri: okuri.map(str::to_owned),
                    text: row.get(0)?,
                    annotation: row.get(1)?,
                    source: CandidateSource::User,
                    recency: Some(effective_frequency(
                        frequency,
                        updated_at,
                        now,
                        self.half_life_seconds,
                    )),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(candidates)
    }

    fn register_at(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
        now: i64,
    ) -> Result<()> {
        let previous = self
            .connection
            .query_row(
                "
                SELECT frequency, updated_at
                FROM candidates
                WHERE input = ?1 AND okuri = ?2 AND text = ?3
                ",
                params![input, okuri.unwrap_or(""), text],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let frequency = previous.map_or(1.0, |(frequency, updated_at)| {
            effective_frequency(frequency, updated_at, now, self.half_life_seconds) + 1.0
        });
        self.connection.execute(
            "
            INSERT INTO candidates (input, okuri, text, annotation, frequency, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT (input, okuri, text)
            DO UPDATE SET
                annotation = excluded.annotation,
                frequency = excluded.frequency,
                updated_at = excluded.updated_at
            ",
            params![input, okuri.unwrap_or(""), text, annotation, frequency, now],
        )?;
        Ok(())
    }

    fn record_usage(&mut self, candidate: &DictionaryCandidate, now: i64) -> Result<()> {
        let previous = self
            .connection
            .query_row(
                "
                SELECT frequency, updated_at
                FROM candidates
                WHERE input = ?1 AND okuri = ?2 AND text = ?3
                ",
                params![
                    candidate.input,
                    candidate.okuri.as_deref().unwrap_or(""),
                    candidate.text
                ],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let frequency = previous.map_or(1.0, |(frequency, updated_at)| {
            effective_frequency(frequency, updated_at, now, self.half_life_seconds) + 1.0
        });
        self.connection.execute(
            "
            INSERT INTO candidates (input, okuri, text, annotation, frequency, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT (input, okuri, text)
            DO UPDATE SET
                annotation = COALESCE(candidates.annotation, excluded.annotation),
                frequency = excluded.frequency,
                updated_at = excluded.updated_at
            ",
            params![
                candidate.input,
                candidate.okuri.as_deref().unwrap_or(""),
                candidate.text,
                candidate.annotation,
                frequency,
                now
            ],
        )?;
        Ok(())
    }

    fn purge(&mut self, input: &str, okuri: Option<&str>, text: &str) -> Result<bool> {
        Ok(self.connection.execute(
            "DELETE FROM candidates
             WHERE input = ?1 AND okuri = ?2 AND text = ?3",
            params![input, okuri.unwrap_or(""), text],
        )? > 0)
    }
}

#[derive(Clone, Debug)]
/// Holds one in-memory candidate's persisted annotation and frequency state.
struct MemoryEntry {
    annotation: Option<String>,
    frequency: f64,
    updated_at: i64,
}

/// Keeps user-dictionary candidates and usage frequencies in memory.
///
/// This implementation is intended for deterministic simulations and tests
/// that must reuse conversion behavior without creating a SQLite database.
#[derive(Clone, Debug)]
pub struct MemoryUserDictionary {
    entries: HashMap<(String, String, String), MemoryEntry>,
    half_life_seconds: f64,
}

impl MemoryUserDictionary {
    /// Creates an empty in-memory user dictionary with the given half-life.
    pub fn new(half_life_days: f64) -> Self {
        Self {
            entries: HashMap::new(),
            half_life_seconds: half_life_days * 86_400.0,
        }
    }
}

impl UserDictionary for MemoryUserDictionary {
    fn set_half_life_days(&mut self, half_life_days: f64) {
        self.half_life_seconds = half_life_days * 86_400.0;
    }

    fn lookup_at(
        &self,
        input: &str,
        okuri: Option<&str>,
        now: i64,
    ) -> Result<Vec<DictionaryCandidate>> {
        let okuri_key = okuri.unwrap_or("");
        Ok(self
            .entries
            .iter()
            .filter(|((entry_input, entry_okuri, _), _)| {
                entry_input == input && entry_okuri == okuri_key
            })
            .map(|((_, _, text), entry)| DictionaryCandidate {
                input: input.to_owned(),
                okuri: okuri.map(str::to_owned),
                text: text.clone(),
                annotation: entry.annotation.clone(),
                source: CandidateSource::User,
                recency: Some(effective_frequency(
                    entry.frequency,
                    entry.updated_at,
                    now,
                    self.half_life_seconds,
                )),
            })
            .collect())
    }

    fn register_at(
        &mut self,
        input: &str,
        okuri: Option<&str>,
        text: &str,
        annotation: Option<&str>,
        now: i64,
    ) -> Result<()> {
        let key = (
            input.to_owned(),
            okuri.unwrap_or("").to_owned(),
            text.to_owned(),
        );
        let frequency = self.entries.get(&key).map_or(1.0, |entry| {
            effective_frequency(
                entry.frequency,
                entry.updated_at,
                now,
                self.half_life_seconds,
            ) + 1.0
        });
        self.entries.insert(
            key,
            MemoryEntry {
                annotation: annotation.map(str::to_owned),
                frequency,
                updated_at: now,
            },
        );
        Ok(())
    }

    fn record_usage(&mut self, candidate: &DictionaryCandidate, now: i64) -> Result<()> {
        let key = (
            candidate.input.clone(),
            candidate.okuri.as_deref().unwrap_or("").to_owned(),
            candidate.text.clone(),
        );
        let previous = self.entries.get(&key);
        let frequency = previous.map_or(1.0, |entry| {
            effective_frequency(
                entry.frequency,
                entry.updated_at,
                now,
                self.half_life_seconds,
            ) + 1.0
        });
        let annotation = previous
            .and_then(|entry| entry.annotation.clone())
            .or_else(|| candidate.annotation.clone());
        self.entries.insert(
            key,
            MemoryEntry {
                annotation,
                frequency,
                updated_at: now,
            },
        );
        Ok(())
    }

    fn purge(&mut self, input: &str, okuri: Option<&str>, text: &str) -> Result<bool> {
        let key = (
            input.to_owned(),
            okuri.unwrap_or("").to_owned(),
            text.to_owned(),
        );
        Ok(self.entries.remove(&key).is_some())
    }
}

/// Returns the current Unix timestamp in seconds for interactive conversion.
pub(crate) fn unix_timestamp() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("システム時刻がUNIX epochより前です")?
        .as_secs();
    i64::try_from(seconds).context("システム時刻が大きすぎます")
}

/// Calculates a stored frequency after applying exponential half-life decay.
fn effective_frequency(frequency: f64, updated_at: i64, now: i64, half_life_seconds: f64) -> f64 {
    let elapsed = now.saturating_sub(updated_at).max(0) as f64;
    frequency * 0.5_f64.powf(elapsed / half_life_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_system_candidate_uses_decayed_frequency() {
        let mut dictionary = SqliteUserDictionary::open(Path::new(":memory:"), 1.0).unwrap();
        let candidate = DictionaryCandidate {
            input: "かな".to_owned(),
            okuri: None,
            text: "仮名".to_owned(),
            annotation: None,
            source: CandidateSource::System,
            recency: None,
        };
        dictionary.record_usage(&candidate, 0).unwrap();
        dictionary.record_usage(&candidate, 0).unwrap();
        let (frequency, updated_at): (f64, i64) = dictionary
            .connection
            .query_row("SELECT frequency, updated_at FROM candidates", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(frequency, 2.0);
        assert_eq!(
            effective_frequency(frequency, updated_at, 86_400, dictionary.half_life_seconds),
            1.0
        );
        let candidates = dictionary.lookup_at("かな", None, 86_400).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].recency, Some(1.0));
    }

    #[test]
    fn registered_candidate_counts_as_a_use() {
        let mut dictionary = SqliteUserDictionary::open(Path::new(":memory:"), 1.0).unwrap();
        dictionary
            .register_at("みだし", None, "登録語", Some("注釈"), 0)
            .unwrap();
        let (annotation, frequency): (String, f64) = dictionary
            .connection
            .query_row("SELECT annotation, frequency FROM candidates", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(annotation, "注釈");
        assert_eq!(frequency, 1.0);
    }

    #[test]
    fn memory_dictionary_matches_decayed_usage_behavior() {
        let mut dictionary = MemoryUserDictionary::new(1.0);
        let candidate = DictionaryCandidate {
            input: "かな".to_owned(),
            okuri: None,
            text: "仮名".to_owned(),
            annotation: Some("annotation".to_owned()),
            source: CandidateSource::System,
            recency: None,
        };

        dictionary.record_usage(&candidate, 0).unwrap();
        dictionary.record_usage(&candidate, 0).unwrap();

        let candidates = dictionary.lookup_at("かな", None, 86_400).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].recency, Some(1.0));
        assert_eq!(candidates[0].annotation.as_deref(), Some("annotation"));
    }
}
