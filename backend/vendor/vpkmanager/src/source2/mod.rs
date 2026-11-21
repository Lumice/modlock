pub mod error; pub mod kv3; pub mod resource; pub use error::{DecodeError,EncodeError};
pub mod texture;
pub fn inspect(bytes:&[u8])->Result<texture::TextureInfo,DecodeError>{let r=resource::Resource::parse(bytes)?; let mut info=texture::parse_texture_header(r.data_block()?)?;info.ycocg=texture::detect_ycocg(&r);Ok(info)}
#[derive(Clone,Copy,Default)] pub struct DecodeOptions {pub mip:u8,pub slice:u16,pub face:u8}
