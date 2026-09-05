//! Bounded, versioned Windows setup state stored as one registry value.

use std::io;

const MAGIC: [u8; 8] = *b"CRWSET01";
const VERSION: u32 = 1;
const MAX_RECORD_BYTES: usize = 32 * 1024;
const MAX_SID_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Status {
    Pending = 1,
    Installed = 2,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct SetupRecord {
    pub(super) status: Status,
    pub(super) owner_sid: Vec<u8>,
    pub(super) account_name: String,
    pub(super) protected_password: Vec<u8>,
    pub(super) account_sid: Vec<u8>,
}

impl SetupRecord {
    pub(super) fn pending(
        owner_sid: Vec<u8>,
        account_name: String,
        protected_password: Vec<u8>,
    ) -> io::Result<Self> {
        let record = Self {
            status: Status::Pending,
            owner_sid,
            account_name,
            protected_password,
            account_sid: Vec::new(),
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn installed(mut self, account_sid: Vec<u8>) -> io::Result<Self> {
        self.status = Status::Installed;
        self.account_sid = account_sid;
        self.validate()?;
        Ok(self)
    }

    pub(super) fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&(self.status as u32).to_le_bytes());
        put_field(&mut bytes, &self.owner_sid)?;
        let account: Vec<u8> = self
            .account_name
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        put_field(&mut bytes, &account)?;
        put_field(&mut bytes, &self.protected_password)?;
        put_field(&mut bytes, &self.account_sid)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid_record());
        }
        Ok(bytes)
    }

    pub(super) fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid_record());
        }
        let mut cursor = Cursor::new(bytes);
        if cursor.take(MAGIC.len())? != MAGIC || cursor.u32()? != VERSION {
            return Err(invalid_record());
        }
        let status = match cursor.u32()? {
            1 => Status::Pending,
            2 => Status::Installed,
            _ => return Err(invalid_record()),
        };
        let owner_sid = cursor.field(MAX_SID_BYTES)?;
        let account = cursor.field(40)?;
        if account.len() % 2 != 0 {
            return Err(invalid_record());
        }
        let account_units = account
            .chunks_exact(2)
            .map(|pair| {
                pair.try_into()
                    .map(u16::from_le_bytes)
                    .map_err(|_| invalid_record())
            })
            .collect::<io::Result<Vec<_>>>()?;
        let account_name = String::from_utf16(&account_units).map_err(|_| invalid_record())?;
        let protected_password = cursor.field(MAX_PASSWORD_BYTES)?;
        let account_sid = cursor.field(MAX_SID_BYTES)?;
        if cursor.remaining() != 0 {
            return Err(invalid_record());
        }
        let record = Self {
            status,
            owner_sid,
            account_name,
            protected_password,
            account_sid,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> io::Result<()> {
        if self.owner_sid.is_empty()
            || self.owner_sid.len() > MAX_SID_BYTES
            || self.account_name.is_empty()
            || self.account_name.len() > 20
            || !self.account_name.is_ascii()
            || self.account_name.contains('\0')
            || self.protected_password.is_empty()
            || self.protected_password.len() > MAX_PASSWORD_BYTES
            || self.account_sid.len() > MAX_SID_BYTES
            || (self.status == Status::Installed && self.account_sid.is_empty())
            || (self.status == Status::Pending && !self.account_sid.is_empty())
        {
            return Err(invalid_record());
        }
        Ok(())
    }
}

fn put_field(bytes: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    bytes.extend_from_slice(
        &u32::try_from(value.len())
            .map_err(|_| invalid_record())?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(value);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(invalid_record)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(invalid_record)?;
        self.position = end;
        Ok(value)
    }

    fn u32(&mut self) -> io::Result<u32> {
        self.take(size_of::<u32>())?
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| invalid_record())
    }

    fn field(&mut self, maximum: usize) -> io::Result<Vec<u8>> {
        let length = usize::try_from(self.u32()?).map_err(|_| invalid_record())?;
        if length > maximum {
            return Err(invalid_record());
        }
        Ok(self.take(length)?.to_vec())
    }
}

fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid Windows sandbox setup record",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(status: Status) -> SetupRecord {
        SetupRecord {
            status,
            owner_sid: vec![1, 2, 3],
            account_name: "CrucibleSBX-12345678".into(),
            protected_password: vec![4, 5, 6],
            account_sid: if status == Status::Installed {
                vec![7, 8, 9]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn pending_and_installed_records_round_trip_exactly() {
        for expected in [record(Status::Pending), record(Status::Installed)] {
            let encoded = expected.encode().expect("encode");
            assert_eq!(SetupRecord::decode(&encoded).expect("decode"), expected);
        }
    }

    #[test]
    fn malformed_or_ambiguous_records_are_rejected() {
        let mut trailing = record(Status::Installed).encode().expect("encode");
        trailing.push(0);
        assert!(SetupRecord::decode(&trailing).is_err());

        let mut wrong_version = record(Status::Installed).encode().expect("encode");
        *wrong_version.get_mut(MAGIC.len()).expect("version byte") = 2;
        assert!(SetupRecord::decode(&wrong_version).is_err());

        let mut missing_sid = record(Status::Installed);
        missing_sid.account_sid.clear();
        assert!(missing_sid.encode().is_err());
    }
}
