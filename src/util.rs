use errors::ParseError;

pub trait Serializable<T> {
    fn from_bytes(bytes: &[u8]) -> Result<T, ParseError>;

    fn to_bytes(&self) -> Result<Vec<u8>, ParseError>;
}
