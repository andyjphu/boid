# Pasted images

Paste a PNG, JPEG, or WebP image into an agent composer. A blue `[Image #1]`
marker represents the attachment. Images upload in the background; sending waits
for those uploads, and the prompt clears only after the server accepts the
message. A message can contain images without text. When images are attached,
text such as `/clear` is literal text, so a command cannot discard the images.

Use the marker's × to remove an attachment. Text editing stays native, and
prompt-history navigation restores the text and images together. Failed uploads
remain visible for retry or removal. A completed upload survives browser refresh
while its agent is available; an upload interrupted by reload must be pasted
again. Browser recovery retains attachment IDs, not image bytes.

Boid permits up to 8 images per message, 10 MiB per image, 20 MiB of images per
message, and 32 million pixels per image. These are Boid resource limits.
Animated images and other formats are rejected. Boid validates images but sends
the original bytes, without resizing, converting, or changing their pixels.
The provider may apply its own image processing and model limits.

An upload alone does not start an agent, create a conversation trace, or reserve
its provisional bird name. Accepted image messages follow the normal ordered
session persistence and restart behavior. Existing text-only records retain
their serialized shape.

## Authenticated HTTP interface

| Request | Body / response |
| --- | --- |
| `POST /api/sessions/{session}/images` | Raw image bytes, with `Content-Type: image/png`, `image/jpeg`, or `image/webp`. Returns HTTP 201 with `{id,mime_type,size_bytes,width,height}`. |
| `GET /api/sessions/{session}/images/{id}` | Original image bytes for a live session. Requires the same authentication as other session routes. |
| `POST /api/sessions/{session}/messages` | `{text:"",images:["image-id",...]}`. `images` is optional. Returns the existing HTTP 202 `{message_id,status:"queued"}` acknowledgment. |

Image IDs belong to their upload session and cannot be submitted to another
session. Accepted transcript entries add an optional `images` array containing
the same metadata plus a per-message label such as `Image #1`. Text remains the
literal submitted text. SSE, recent-history pages, and peer trace reads contain
this metadata, not encoded image bytes. Peer trace inspection does not transfer
image pixels into another agent's model context.

## Storage and request construction

Private blobs live in `images/{session}/{sha256}.bin` under the Boid state
directory, with a small `.json` metadata sidecar. Bytes become durable before an
upload is acknowledged. Queued messages and model history retain references.
Image-bearing sessions use record version 5; older binaries reject that version
rather than silently losing images. Existing records remain readable.

Before an inference request, Boid resolves references outside the session mutex
and networking worker. Heavy decoding and image materialization share one work
slot. Text-only requests perform no image file I/O. The API and subscription
routes receive actual `input_image` items with base64 data URLs, the transport
format described in the [Responses image-input guide](https://developers.openai.com/api/docs/guides/images-vision?api-mode=responses).
HTTP retries reuse the encoded request body. When compaction retains an image,
Boid stores its returned bytes as a blob reference without changing the returned
conversation content. Subscription compaction remains unavailable in this
integration, as before.

Ending an agent leaves its image blobs available for manual reading alongside
its archived trace. The harness does not serve archived sessions through the
image endpoint. Unsubmitted uploads are also retained; there is no automatic
image garbage collection in this version.
