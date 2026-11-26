// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

const OWNERS_FILE: &str = ".gaze-owners";

#[derive(Debug, PartialEq, Eq)]
pub enum OwnerCheck {
    Current,
    Reassigned { previous_uid: u32 },
}

impl UserDatabase {
    fn owners_path(&self) -> std::io::Result<PathBuf> {
        self.base_dir
            .parent()
            .map(|parent| parent.join(OWNERS_FILE))
            .ok_or_else(|| std::io::Error::other("store has no parent"))
    }

    fn read_owners(&self) -> std::io::Result<HashMap<String, u32>> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.owners_path()?)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(error) => return Err(error),
        };
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)?;
        let mut owners = HashMap::new();
        for line in contents.lines().filter(|line| !line.is_empty()) {
            let (uid, username) = line
                .split_once('\t')
                .and_then(|(uid, name)| Some((uid.parse::<u32>().ok()?, name)))
                .ok_or_else(|| std::io::Error::other(format!("malformed {OWNERS_FILE} entry")))?;
            owners.insert(username.to_string(), uid);
        }
        Ok(owners)
    }

    fn write_owners(&self, owners: &HashMap<String, u32>) -> std::io::Result<()> {
        let path = self.owners_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("store has no parent"))?;
        let mut entries: Vec<_> = owners.iter().collect();
        entries.sort();
        let contents: String = entries
            .into_iter()
            .map(|(username, uid)| format!("{uid}\t{username}\n"))
            .collect();
        let tmp = parent.join(format!("{OWNERS_FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&tmp, &path))
        {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
        sync_dir(parent)
    }

    pub fn reconcile_owner(&mut self, username: &str, uid: u32) -> Result<OwnerCheck, UserDbError> {
        self.ensure_available()?;
        Self::validate_username(username)?;
        let mut owners = self.read_owners()?;
        let user_dir = self.user_dir(username);
        let previous_uid = match owners.get(username) {
            Some(&recorded) if recorded == uid => return Ok(OwnerCheck::Current),
            Some(&recorded) => Some(recorded),
            None => None,
        };
        let has_templates = fs::symlink_metadata(&user_dir).is_ok();
        if let Some(previous_uid) = previous_uid {
            if has_templates {
                Self::retire_dir(&user_dir)?;
            }
            self.users.remove(username);
            owners.insert(username.to_string(), uid);
            self.write_owners(&owners)?;
            return Ok(OwnerCheck::Reassigned { previous_uid });
        }
        if has_templates {
            owners.insert(username.to_string(), uid);
            self.write_owners(&owners)?;
        }
        Ok(OwnerCheck::Current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::tests::{TempDir, embedding, rgb_embeds};

    fn store(temp: &TempDir) -> UserDatabase {
        let base = temp.path().join("users");
        UserDatabase::new(base.to_str().unwrap(), 4).unwrap()
    }

    fn enroll(db: &mut UserDatabase, username: &str) {
        db.add_template(
            username,
            "face",
            "template",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
    }

    #[test]
    fn the_enrolling_account_keeps_its_templates() {
        let temp = TempDir::new("owner-keep");
        let mut db = store(&temp);
        enroll(&mut db, "bob");
        assert_eq!(
            db.reconcile_owner("bob", 1500).unwrap(),
            OwnerCheck::Current
        );
        assert_eq!(
            db.reconcile_owner("bob", 1500).unwrap(),
            OwnerCheck::Current
        );
        assert_eq!(db.list_faces("bob").unwrap().len(), 1);
    }

    #[test]
    fn a_recreated_account_does_not_inherit_templates() {
        let temp = TempDir::new("owner-reassign");
        let mut db = store(&temp);
        enroll(&mut db, "bob");
        db.reconcile_owner("bob", 1500).unwrap();

        assert_eq!(
            db.reconcile_owner("bob", 1600).unwrap(),
            OwnerCheck::Reassigned { previous_uid: 1500 }
        );
        assert!(db.list_faces("bob").is_err());
        assert!(!temp.path().join("users/bob").exists());

        let reopened = store(&temp);
        assert!(reopened.list_faces("bob").is_err());
        let mut reopened = reopened;
        assert_eq!(
            reopened.reconcile_owner("bob", 1600).unwrap(),
            OwnerCheck::Current
        );
    }

    #[test]
    fn users_without_templates_are_not_recorded() {
        let temp = TempDir::new("owner-empty");
        let mut db = store(&temp);
        assert_eq!(
            db.reconcile_owner("eve", 1700).unwrap(),
            OwnerCheck::Current
        );
        assert!(!temp.path().join(OWNERS_FILE).exists());
        enroll(&mut db, "eve");
        assert_eq!(
            db.reconcile_owner("eve", 1700).unwrap(),
            OwnerCheck::Current
        );
        assert_eq!(db.read_owners().unwrap().get("eve"), Some(&1700));
    }

    #[test]
    fn owner_records_survive_names_with_spaces_and_reject_corruption() {
        let temp = TempDir::new("owner-format");
        let mut db = store(&temp);
        enroll(&mut db, "Alice Smith");
        db.reconcile_owner("Alice Smith", 1800).unwrap();
        assert_eq!(db.read_owners().unwrap().get("Alice Smith"), Some(&1800));

        fs::write(temp.path().join(OWNERS_FILE), "not-a-uid\talice\n").unwrap();
        assert!(db.reconcile_owner("Alice Smith", 1800).is_err());
    }
}
