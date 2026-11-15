use std::{
    io::{self, Cursor, Write},
    sync::OnceLock,
};

use simdnbt::owned::{NbtCompound, NbtTag};

use crate::{
    codec::VarInt,
    hash::{ComponentHasher, HashComponent},
    serial::{ReadFrom, WriteTo},
};

/// A placeholder type for unimplemented component values.
/// Unlike `()`, this is a distinct type that can have its own trait implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Todo;

impl WriteTo for Todo {
    fn write(&self, _writer: &mut impl Write) -> io::Result<()> {
        // Placeholder components write nothing
        Ok(())
    }
}

impl ReadFrom for Todo {
    fn read(_data: &mut Cursor<&[u8]>) -> io::Result<Self> {
        // Placeholder components read nothing
        Ok(Todo)
    }
}

impl HashComponent for Todo {
    fn hash_component(&self, hasher: &mut ComponentHasher) {
        // Hash as empty value
        hasher.put_empty();
    }
}

impl simdnbt::ToNbtTag for Todo {
    fn to_nbt_tag(self) -> NbtTag {
        // Placeholder components serialize as empty compound
        NbtTag::Compound(NbtCompound::new())
    }
}

impl simdnbt::FromNbtTag for Todo {
    fn from_nbt_tag(_tag: simdnbt::borrow::NbtTag) -> Option<Self> {
        // Placeholder components always deserialize successfully
        Some(Todo)
    }
}

/// A raw block state id. Using the registry this id can be derived into a block and it's current properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockStateId(pub u16);

/// What clients are sent for block states their game does not have.
struct ClientBlockStates {
    /// The state shown for each server state, indexed by server state id.
    shown: Box<[u16]>,
    /// How many block states the client's game has.
    count: u16,
}

static CLIENT_BLOCK_STATES: OnceLock<ClientBlockStates> = OnceLock::new();

/// Makes every block state go to clients as `shown[state]`, for a server whose block
/// registry is larger than the client's. `count` is the number of states the client has.
///
/// Returns `false` if the table was already set; it can be set once.
pub fn set_client_block_states(shown: Box<[u16]>, count: u16) -> bool {
    CLIENT_BLOCK_STATES
        .set(ClientBlockStates { shown, count })
        .is_ok()
}

/// How many block states the client's game has, if it differs from the server's registry.
#[must_use]
pub fn client_block_state_count() -> Option<u16> {
    CLIENT_BLOCK_STATES.get().map(|states| states.count)
}

impl BlockStateId {
    /// The state a client is sent for this one: itself, unless
    /// [`set_client_block_states`] says otherwise.
    #[must_use]
    pub fn to_client(self) -> Self {
        CLIENT_BLOCK_STATES
            .get()
            .and_then(|states| states.shown.get(usize::from(self.0)))
            .map_or(self, |shown| Self(*shown))
    }
}

impl WriteTo for BlockStateId {
    /// Writes the state for a client, see [`BlockStateId::to_client`].
    fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        VarInt(i32::from(self.to_client().0)).write(writer)
    }
}

impl ReadFrom for BlockStateId {
    fn read(data: &mut Cursor<&[u8]>) -> io::Result<Self> {
        let id = VarInt::read(data)?.0;
        #[expect(
            clippy::cast_sign_loss,
            reason = "VarInt is validated upstream; block state IDs are non-negative"
        )]
        Ok(Self(id as u16))
    }
}
