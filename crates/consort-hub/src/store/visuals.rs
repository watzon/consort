//! The image cache of visuals: one row for each image that the session host
//! drew (`visual_images`). The PNG is a blob. The rows of a message go when
//! the message goes.

use consort_proto::ids::MessageId;
use consort_proto::model::Sha256Hash;
use rusqlite::{Connection, OptionalExtension, params};

use super::Tx;
use crate::error::{HubError, Result};

/// The most cached images of one visual. When a new image would be the one
/// after this, the oldest image of the visual goes. A client picks its
/// widths, so the number of images is not fixed by the visual.
pub const IMAGES_PER_MESSAGE_MAX: i64 = 32;

/// One row of the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedImage {
    /// The blob of the PNG.
    pub image_sha256: Sha256Hash,
    /// The size of the PNG in bytes.
    pub size: u64,
    /// The height of the page in CSS pixels.
    pub height: u32,
}

/// The row with this key.
pub fn image(conn: &Connection, key: &str) -> Result<Option<CachedImage>> {
    let row = conn
        .query_row(
            "SELECT image_sha256, size, height FROM visual_images WHERE key = ?1",
            [key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((sha, size, height)) = row else {
        return Ok(None);
    };
    // A row that does not read back is a row that was never valid, so the
    // caller draws the image again and the new row replaces it.
    let (Ok(image_sha256), Ok(size), Ok(height)) = (
        sha.parse::<Sha256Hash>(),
        u64::try_from(size),
        u32::try_from(height),
    ) else {
        return Ok(None);
    };
    Ok(Some(CachedImage {
        image_sha256,
        size,
        height,
    }))
}

/// How many images the cache holds for a message.
#[cfg(test)]
pub fn image_count(conn: &Connection, message_id: MessageId) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM visual_images WHERE message_id = ?1",
        [message_id.to_string()],
        |row| row.get(0),
    )?)
}

impl Tx<'_> {
    /// Saves an image under its key. A row with this key is replaced. Then
    /// the oldest images of the message go, so that at most
    /// [`IMAGES_PER_MESSAGE_MAX`] stay.
    pub fn save_visual_image(
        &mut self,
        key: &str,
        message_id: MessageId,
        image: &CachedImage,
    ) -> Result<()> {
        let size = i64::try_from(image.size)
            .map_err(|_| HubError::internal("the size of a visual image is out of range"))?;
        let message = message_id.to_string();
        self.db().execute(
            "INSERT INTO visual_images (key, message_id, image_sha256, size, height, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(key) DO UPDATE SET
                message_id = excluded.message_id,
                image_sha256 = excluded.image_sha256,
                size = excluded.size,
                height = excluded.height,
                created_at = excluded.created_at",
            params![
                key,
                message,
                image.image_sha256.as_str(),
                size,
                i64::from(image.height),
                self.now().as_millis(),
            ],
        )?;
        self.db().execute(
            "DELETE FROM visual_images WHERE message_id = ?1 AND key NOT IN (
                SELECT key FROM visual_images WHERE message_id = ?1
                ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
            params![message, IMAGES_PER_MESSAGE_MAX],
        )?;
        Ok(())
    }

    /// Deletes the cached images of a message. The blobs stay: the hub
    /// keeps every blob.
    pub fn delete_visual_images(&mut self, message_id: MessageId) -> Result<usize> {
        Ok(self.db().execute(
            "DELETE FROM visual_images WHERE message_id = ?1",
            [message_id.to_string()],
        )?)
    }
}
