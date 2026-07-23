use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::{
    ClientConfig, ControlError, ControlFrame, Join, NamespacedRoomKey, RendezvousError, Reply,
    Role, WaitKind, read_control, write_control,
};

pub async fn join_room<R, W>(
    reader: &mut R,
    writer: &mut W,
    room_key: NamespacedRoomKey,
    config: ClientConfig,
) -> Result<Role, RendezvousError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    timeout(config.reply_deadline(), async {
        write_control(
            writer,
            &ControlFrame::Join(Join::new(room_key)),
            config.control(),
        )
        .await?;
        match read_control(reader, config.control()).await? {
            ControlFrame::Reply(Reply::Paired(paired)) => Ok(paired.role),
            ControlFrame::Reply(Reply::Expired) => Err(RendezvousError::Expired),
            ControlFrame::Reply(Reply::Rejected(reason)) => Err(RendezvousError::Rejected(reason)),
            ControlFrame::Join(_) => Err(ControlError::UnexpectedFrame.into()),
        }
    })
    .await
    .map_err(|_| RendezvousError::Deadline {
        wait: WaitKind::Reply,
    })?
}
