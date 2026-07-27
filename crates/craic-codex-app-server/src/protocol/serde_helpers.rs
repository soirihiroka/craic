use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(super) fn deserialize_double_option<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(super) fn serialize_double_option<S, T>(
    value: &Option<Option<T>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    value
        .as_ref()
        .and_then(Option::as_ref)
        .serialize(serializer)
}
