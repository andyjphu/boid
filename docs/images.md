# Image attachments

Paste a screenshot or image into an agent's composer. Each attachment appears
as a blue `[Image #N]` marker with a removal control. Text editing stays native.
Enter sends the text and attached images, including an image-only message.
Prompt-history navigation restores the text and images together.

Uploads run in the background. A message waits for all of its attachments;
failed uploads remain visible for retry or removal. An attachment whose upload
was interrupted by a page reload must be pasted again. Completed uploads survive
browser refresh while the agent remains available. The composer clears only
after the server acknowledges its exact submitted draft.

Boid accepts static PNG, JPEG and WebP images, preserving their original bytes.
Application limits are eight images per message, 10 MiB per image, 20 MiB total
per message, and 32 million pixels per image. Unsupported or invalid images are
rejected. These are Boid's resource limits, not statements of provider limits.

Images are private files under the server's state directory:
`images/<session-id>/<sha256>.bin`, with adjacent JSON metadata. Queued input,
transcripts and stored model history retain small references. Provider requests
resolve those references into Responses API `input_image` content; both API and
subscription routes receive the image bytes. Routine browser events and session
snapshots do not carry base64 images.

Uploading alone does not start an empty agent, record a conversation, or reserve
its automatic name. Unused uploads remain on disk for now. Ending a used agent
retains its image files with the archived conversation; the normal image API
does not expose ended sessions. Image-bearing records use schema version 5 so
older binaries reject them instead of silently omitting attachments. Preserve
the entire state directory when backing up or rolling back a release.

The authenticated API accepts a raw image body at
`POST /api/sessions/{session-id}/images` with its image Content-Type. The returned
metadata includes an image ID. Submit `{ "text": "...", "images": ["id"] }`
to the existing session messages endpoint. IDs belong to the session that
uploaded them. `GET /api/sessions/{session-id}/images/{image-id}` returns the
original bytes for a live session. Peer trace inspection includes attachment
metadata; it does not transfer the pixels into another agent's model context.
