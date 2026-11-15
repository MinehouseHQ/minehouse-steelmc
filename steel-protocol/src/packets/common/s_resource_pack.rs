use steel_macros::{ReadFrom, ServerPacket};
use uuid::Uuid;

/// What the client did with an offered resource pack. Vanilla
/// `ServerboundResourcePackPacket.Action`.
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum ResourcePackAction {
    /// The pack is in use.
    SuccessfullyLoaded = 0,
    /// The player refused the pack.
    Declined = 1,
    /// The pack could not be downloaded.
    FailedDownload = 2,
    /// The player agreed; the download starts.
    Accepted = 3,
    /// The pack is downloaded and is being applied.
    Downloaded = 4,
    /// The client could not read the address of the pack.
    InvalidUrl = 5,
    /// The pack was downloaded but could not be applied.
    FailedReload = 6,
    /// The pack was dropped before it was applied.
    Discarded = 7,
}

impl ResourcePackAction {
    /// Whether this is the client's last word about the pack. Vanilla `Action.isTerminal`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Accepted | Self::Downloaded)
    }
}

/// Vanilla `ServerboundResourcePackPacket`: the client's progress with an offered pack.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
pub struct SResourcePack {
    /// The pack the answer is about.
    pub id: Uuid,
    /// What happened to it.
    pub action: ResourcePackAction,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::serial::ReadFrom as _;

    use super::*;

    #[test]
    fn answer_reads_the_id_then_the_action() {
        let mut bytes = vec![0; 15];
        bytes.push(9);
        bytes.push(3);
        let packet = SResourcePack::read(&mut Cursor::new(&bytes[..])).expect("should decode");
        assert_eq!(packet.id, Uuid::from_u128(9));
        assert_eq!(packet.action, ResourcePackAction::Accepted);
    }

    #[test]
    fn only_the_download_steps_are_not_terminal() {
        assert!(!ResourcePackAction::Accepted.is_terminal());
        assert!(!ResourcePackAction::Downloaded.is_terminal());
        for action in [
            ResourcePackAction::SuccessfullyLoaded,
            ResourcePackAction::Declined,
            ResourcePackAction::FailedDownload,
            ResourcePackAction::InvalidUrl,
            ResourcePackAction::FailedReload,
            ResourcePackAction::Discarded,
        ] {
            assert!(action.is_terminal());
        }
    }
}
