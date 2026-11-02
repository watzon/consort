//! `visual.render`: the image of a visual, from the hub.

use consort_proto::client_api::VisualRender;

use super::{RENDER_LIMIT, Shared};
use crate::{
    ClientError,
    visual::{Turn, VisualImageFile, VisualKey, render_error},
};

impl Shared {
    /// The image of a visual at one width, scale and theme. An image that
    /// this run got before is answered with no request. Two asks for the same
    /// image at once share one request. An error is not kept.
    pub(crate) async fn visual_image(
        &self,
        message_id: String,
        width: u32,
        scale: u8,
        style: consort_proto::model::VisualStyle,
    ) -> Result<VisualImageFile, ClientError> {
        let key = VisualKey {
            message_id,
            width,
            scale,
            style,
        };
        loop {
            let turn = self
                .visual_images
                .lock()
                .map_err(|_| ClientError::Other("The visual cache is not usable".into()))?
                .begin(&key);
            match turn {
                Turn::Ready(file) => return Ok(file),
                Turn::Wait(mut rx) => {
                    loop {
                        if let Some(result) = rx.borrow_and_update().clone() {
                            return result;
                        }
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                    // The ask that sent the request is gone. Ask again.
                }
                Turn::Render(tx) => {
                    let generation = self.generation();
                    let result = self.render_once(&key).await;
                    // An image that came after Pair or Unpair belongs to
                    // the old hub, so it is not kept.
                    let keep = self.generation() == generation;
                    if let Ok(mut images) = self.visual_images.lock() {
                        images.finish(&key, &tx, &result, keep);
                    }
                    return result;
                }
            }
        }
    }

    /// Sends `visual.render` and downloads the PNG that the hub names.
    async fn render_once(&self, key: &VisualKey) -> Result<VisualImageFile, ClientError> {
        let answer = self
            .call_within(
                VisualRender {
                    message_id: super::commands::parse_id(&key.message_id, "message")?,
                    width: key.width,
                    scale: key.scale,
                    style: key.style.clone(),
                },
                self.options.timeouts.call.max(RENDER_LIMIT),
            )
            .await
            .map_err(render_error)?;
        let path = self
            .fetch_blob(&answer.image.sha256.to_string(), answer.image.size)
            .await?;
        Ok(VisualImageFile {
            path: path.display().to_string(),
            width: key.width,
            height: answer.height,
            scale: key.scale,
        })
    }
}
