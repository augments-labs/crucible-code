//! Reading and writing the JSON every value here travels as.
//!
//! One object at a time, and strictly: [`Fields`] hands out each field once
//! and [`Fields::done`] refuses whatever is left, so a field this build does
//! not know is a refusal rather than something silently dropped. Nothing here
//! echoes what it read — every failure is an [`ErrorCode`].
//!
//! A frame is hostile until it has been read, so what bounds it is enforced
//! while it is read and not afterwards. Its length is looked at before a byte
//! is parsed; then [`Reading`] builds the value tree and refuses, at the entry
//! that goes over, a list or an object with more than [`ITEMS`] entries and
//! anything nested more than [`DEPTH`] deep. Those two bound each list and not
//! their product, so one count is kept across the whole read as well: every
//! value built spends one of [`VALUES`], and the value that would go over
//! refuses the frame. A frame of a million one-byte entries therefore costs
//! [`ITEMS`] values where they sit in one list and [`VALUES`] where they are
//! nested, and never a million. A key said twice in one object is refused
//! there too, because which of the two a reader believes is otherwise an
//! accident of the parser. [`frame`] holds what is written to the same counts,
//! so this build never sends what it would refuse to read.

use std::cell::Cell;
use std::fmt;

use serde_core::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::bounds::{DEPTH, FRAME_BYTES, ITEMS, Name, Said, Text, VALUES};
use crate::error::{ErrorCode, Refusal};

/// The word an enum's arm is named by on the wire.
pub(crate) const KIND: &str = "kind";

/// One JSON object, read field by field.
pub(crate) struct Fields(Map<String, Value>);

impl Fields {
    /// The object `value` is.
    pub(crate) fn of(value: Value) -> Result<Self, Refusal> {
        match value {
            Value::Object(map) => Ok(Self(map)),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }

    /// The field `key`, which has to be there.
    pub(crate) fn take(&mut self, key: &str) -> Result<Value, Refusal> {
        self.0.remove(key).ok_or(ErrorCode::Malformed.into())
    }

    /// The field `key`, where it is there and is not `null`.
    pub(crate) fn maybe(&mut self, key: &str) -> Option<Value> {
        self.0.remove(key).filter(|value| !value.is_null())
    }

    /// A string field, by reference to nothing: the caller bounds it.
    pub(crate) fn string(&mut self, key: &str) -> Result<String, Refusal> {
        match self.take(key)? {
            Value::String(text) => Ok(text),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }

    /// The word naming which arm of an enum this object is.
    pub(crate) fn kind(&mut self) -> Result<String, Refusal> {
        self.string(KIND)
    }

    /// A [`Name`] field.
    pub(crate) fn name(&mut self, key: &str) -> Result<Name, Refusal> {
        Name::new(&self.string(key)?)
    }

    /// A [`Text`] field.
    pub(crate) fn text(&mut self, key: &str) -> Result<Text, Refusal> {
        text(self.take(key)?)
    }

    /// A [`Said`] field.
    pub(crate) fn said(&mut self, key: &str) -> Result<Said, Refusal> {
        said(&self.take(key)?)
    }

    /// A whole-number field.
    pub(crate) fn number(&mut self, key: &str) -> Result<u64, Refusal> {
        self.take(key)?.as_u64().ok_or(ErrorCode::Malformed.into())
    }

    /// A whole-number field that may be absent.
    pub(crate) fn maybe_number(&mut self, key: &str) -> Result<Option<u64>, Refusal> {
        self.maybe(key)
            .map(|value| value.as_u64().ok_or(ErrorCode::Malformed.into()))
            .transpose()
    }

    /// A yes-or-no field.
    pub(crate) fn flag(&mut self, key: &str) -> Result<bool, Refusal> {
        self.take(key)?.as_bool().ok_or(ErrorCode::Malformed.into())
    }

    /// A list field, refused over [`ITEMS`] entries before any is read.
    pub(crate) fn list(&mut self, key: &str) -> Result<Vec<Value>, Refusal> {
        match self.take(key)? {
            Value::Array(items) if items.len() <= ITEMS => Ok(items),
            Value::Array(_) => Err(ErrorCode::TooLarge.into()),
            _ => Err(ErrorCode::Malformed.into()),
        }
    }

    /// Closes the object: a field nobody asked for refuses the frame.
    pub(crate) fn done(self) -> Result<(), Refusal> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ErrorCode::Malformed.into())
        }
    }
}

/// One JSON object, written field by field.
pub(crate) struct Writing(Map<String, Value>);

impl Writing {
    /// An object for the arm `kind` of an enum.
    pub(crate) fn kind(kind: &str) -> Self {
        Self::new().with(KIND, kind)
    }

    /// An object with nothing in it yet.
    pub(crate) fn new() -> Self {
        Self(Map::new())
    }

    /// With `key` holding `value`.
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    /// With `key` holding `value` where there is one.
    pub(crate) fn maybe(self, key: &str, value: Option<impl Into<Value>>) -> Self {
        match value {
            Some(value) => self.with(key, value),
            None => self,
        }
    }

    /// With `key` holding `text`.
    pub(crate) fn text(self, key: &str, text: &Text) -> Self {
        self.with(key, written(text))
    }

    /// The object.
    pub(crate) fn finish(self) -> Value {
        Value::Object(self.0)
    }
}

/// A [`Text`] as it travels: the words, and whether they were cut.
pub(crate) fn written(text: &Text) -> Value {
    Writing::new()
        .with("text", text.as_str())
        .with("truncated", text.truncated())
        .finish()
}

/// The [`Text`] `value` is.
pub(crate) fn text(value: Value) -> Result<Text, Refusal> {
    let mut fields = Fields::of(value)?;
    let words = fields.string("text")?;
    let truncated = fields.flag("truncated")?;
    fields.done()?;
    Text::arrived(&words, truncated)
}

/// The [`Said`] `value` is: a string, whole.
pub(crate) fn said(value: &Value) -> Result<Said, Refusal> {
    Said::new(value.as_str().ok_or(ErrorCode::Malformed)?)
}

/// Whether `value` holds no list or object over [`ITEMS`] entries, nests no
/// deeper than `room` more levels, and is made of no more values than `left`
/// has, which it is spent from.
fn fits(value: &Value, room: usize, left: &mut usize) -> bool {
    let Some(rest) = left.checked_sub(1) else {
        return false;
    };
    *left = rest;

    let within = |len: usize| len <= ITEMS && room > 0;
    let inside = room.saturating_sub(1);
    match value {
        Value::Array(items) => {
            within(items.len()) && items.iter().all(|item| fits(item, inside, left))
        }
        Value::Object(map) => {
            within(map.len()) && map.values().all(|item| fits(item, inside, left))
        }
        _ => true,
    }
}

/// The bytes of `value`, refused where a reader here would refuse them: over
/// the entry, nesting or value ceilings, or too long for one frame.
pub(crate) fn frame(value: &Value) -> Result<Vec<u8>, Refusal> {
    if !fits(value, DEPTH, &mut { VALUES }) {
        return Err(ErrorCode::TooLarge.into());
    }
    let bytes = serde_json::to_vec(value).map_err(|_| Refusal::new(ErrorCode::Malformed))?;
    if bytes.len() > FRAME_BYTES {
        return Err(ErrorCode::TooLarge.into());
    }
    Ok(bytes)
}

/// The value `bytes` spell: refused by length before anything parses them, and
/// by [`Reading`] while something does.
pub(crate) fn parsed(bytes: &[u8]) -> Result<Value, Refusal> {
    if bytes.len() > FRAME_BYTES {
        return Err(ErrorCode::TooLarge.into());
    }

    let why = Cell::new(None);
    let left = Cell::new(VALUES);
    let mut reader = serde_json::Deserializer::from_slice(bytes);
    let read = Reading {
        room: DEPTH,
        left: &left,
        why: &why,
    }
    .deserialize(&mut reader)
    .and_then(|value| reader.end().map(|()| value));

    // The reader's own error says where and what, which is the frame's
    // contents; only the code kept beside it leaves.
    read.map_err(|_| why.take().unwrap_or(ErrorCode::Malformed).into())
}

/// Builds one value of a frame, counting as it goes.
///
/// `room` is how many more lists or objects may open inside this value, and
/// `left` how many more values the whole frame may still be made of: one count,
/// shared by every value of the read. `why` is where a refusal that is not
/// "malformed" is left, because the reader's error type carries a sentence and
/// this protocol's refusals carry a code.
#[derive(Clone, Copy)]
struct Reading<'a> {
    room: usize,
    left: &'a Cell<usize>,
    why: &'a Cell<Option<ErrorCode>>,
}

impl Reading<'_> {
    /// Stops the reader, for `code`.
    fn refuse<E: serde_core::de::Error>(self, code: ErrorCode) -> E {
        self.why.set(Some(code));
        E::custom("refused")
    }

    /// Spends the one value about to be built, where the frame has one left.
    fn spend<E: serde_core::de::Error>(self) -> Result<(), E> {
        match self.left.get().checked_sub(1) {
            Some(left) => {
                self.left.set(left);
                Ok(())
            }
            None => Err(self.refuse(ErrorCode::TooLarge)),
        }
    }

    /// What reads a value one level further in, where there is room for one.
    fn inside<E: serde_core::de::Error>(self) -> Result<Self, E> {
        match self.room.checked_sub(1) {
            Some(room) => Ok(Self { room, ..self }),
            None => Err(self.refuse(ErrorCode::TooLarge)),
        }
    }
}

impl<'de> DeserializeSeed<'de> for Reading<'_> {
    type Value = Value;

    fn deserialize<D: serde_core::Deserializer<'de>>(self, from: D) -> Result<Value, D::Error> {
        from.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Reading<'_> {
    type Value = Value;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("a JSON value")
    }

    fn visit_bool<E: serde_core::de::Error>(self, value: bool) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: serde_core::de::Error>(self, value: i64) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: serde_core::de::Error>(self, value: u64) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: serde_core::de::Error>(self, value: f64) -> Result<Value, E> {
        self.spend()?;
        Ok(Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: serde_core::de::Error>(self, value: &str) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: serde_core::de::Error>(self, value: String) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::String(value))
    }

    fn visit_unit<E: serde_core::de::Error>(self) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut list: A) -> Result<Value, A::Error> {
        self.spend()?;
        let inside = self.inside()?;
        let mut items = Vec::new();

        while items.len() < ITEMS {
            match list.next_element_seed(inside)? {
                Some(item) => items.push(item),
                None => return Ok(Value::Array(items)),
            }
        }

        // Full. One more entry is looked for without being built.
        match list.next_element::<IgnoredAny>()? {
            Some(IgnoredAny) => Err(self.refuse(ErrorCode::TooLarge)),
            None => Ok(Value::Array(items)),
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        self.spend()?;
        let inside = self.inside()?;
        let mut fields = Map::new();

        while fields.len() < ITEMS {
            let Some(key) = object.next_key::<String>()? else {
                return Ok(Value::Object(fields));
            };
            if fields.contains_key(&key) {
                return Err(A::Error::custom("a key said twice"));
            }
            let value = object.next_value_seed(inside)?;
            fields.insert(key, value);
        }

        match object.next_key::<IgnoredAny>()? {
            Some(IgnoredAny) => Err(self.refuse(ErrorCode::TooLarge)),
            None => Ok(Value::Object(fields)),
        }
    }
}
