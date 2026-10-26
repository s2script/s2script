---
"@s2script/sdk": minor
---

Read bones and attachments. `EntityRef` in `@s2script/sdk/entity` gains `lookupBone(name)`,
`getBoneTransform(bone)` (world position and orientation quaternion), `lookupAttachment(name)` and
`getAttachment(attachment)` (world origin and angles), like ModSharp's and Swiftly's bone and
attachment APIs. Useful for head positions, muzzle points and attaching effects.
