/** zones — English seed. Generated into translations/zones.phrases.json by scripts/gen-phrases.mjs.
 *
 * The original never imported ChatColors and never used an "[SM] " prefix anywhere — every reply
 * here is bare operator/diagnostic text (sm_zone_* is admin-console tooling, not player chat), so
 * per the "colour only where the original had a [SM] prefix or explicit colour" rule, NONE of these
 * phrases carry a colour tag. All of them ship on expanding paths (Chat.toSlot / cmd.replyT), so a
 * tag would render fine if added — it's simply not warranted by anything in the source.
 */
export const phrases = {
  // --- in-game E-mark editor (Chat.toSlot; keeps the "[zones] " prefix as literal text, same as
  // the original — matching basevotes' "[Vote] " precedent for a plugin tag with no colour). The
  // Creating/Editing verb is baked into two separate keys rather than passed as an argument, so a
  // translator can render the whole sentence naturally instead of splicing in a raw English word.
  "Edit Start Creating": "[zones] Creating zone '{1}': walk to a corner and press E; press E again at the opposite corner. (60s timeout; sm_zone_edit cancel to abort)",
  "Edit Start Editing": "[zones] Editing zone '{1}': walk to a corner and press E; press E again at the opposite corner. (60s timeout; sm_zone_edit cancel to abort)",
  "Edit Timed Out": "[zones] Edit session timed out.",
  "Edit No Pawn": "[zones] Edit session cancelled (no pawn).",
  "Edit No Position Retry": "[zones] No position — try again.",
  "Edit Corner1 Set": "[zones] Corner 1 set — walk to the opposite corner and press E.",
  "Edit Zero Volume": "[zones] Zero-volume box — move further from corner 1 and press E again.",
  "Edit Zone Saved": "[zones] Zone '{1}' saved.",
  // Differs from "Zone Add Save Failed" below only by the "[zones] " prefix — this one is the
  // Chat.toSlot path from the in-game editor, that one is cmd.reply from sm_zone_add; the original
  // had two distinct strings and this keeps them distinct.
  "Edit Save Failed": "[zones] Save failed: {1}",

  // --- sm_zone_add ---
  "Usage Zone Add": "Usage: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>  |  sm_zone_add <name> [size]  |  sm_zone_add <name> (in-game: mark corners with E)",
  "Invalid Coordinates": "Invalid coordinates.",
  "Zone Add Console Only": "From the console, give explicit coords: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>",
  "No Position Spawn Coords": "No position — spawn in first, or give explicit coords.",
  "Zero Volume Zone Rejected": "Zero-volume zone rejected.",
  "Zone Add Saved": "Zone '{1}' saved ({2})-({3})",
  "Zone Add Save Failed": "Save failed: {1}",

  // --- sm_zone_edit ---
  "Zone Edit Ingame Only": "sm_zone_edit is in-game only (it marks corners at your position).",
  "Zone Edit Cancelled": "Zone edit cancelled.",
  "Usage Zone Edit": "Usage: sm_zone_edit <name>  |  sm_zone_edit cancel",
  "Invalid Zone Name": "Invalid zone name.",
  "No Position Spawn First": "No position — spawn in first.",

  // --- sm_zone_delete (also reused by sm_zone_show for the identical "not found" line) ---
  "Zone Not Found": "No zone '{1}' on this map.",
  "Zone Deleted": "Zone '{1}' deleted.",

  // --- sm_zone_tag ---
  "Zone Tag Not Found": "No zone '{1}' on this map. Usage: sm_zone_tag <name> [tag...] (no tags = clear)",
  "Zone Tags Set": "Zone '{1}' tags: {2}",
  "Zone Tags Cleared": "Zone '{1}' tags cleared.",

  // --- sm_zone_list ---
  "Zone List Header Tagged": "Zones on {1} tagged '{2}': {3}",
  "Zone List Header All": "Zones on {1}: {2}",
  // The leading "  " is significant — it's the list-row indent under the header above, same as
  // adminhelp's "Command Row". A translation that trims leading whitespace flattens that indent.
  // trigger={6}'s value is "yes"/"pending", passed as a plain untranslated string — see the comment
  // at the sm_zone_list call site in plugin.ts for why (identifier, not prose; the "trigger=" label
  // itself is hardcoded English right here, so translating only the value would language-mix this
  // diagnostic line and break log-grepping tooling).
  "Zone List Row": "  {1} ({2})-({3}) tags=[{4}] inside={5} trigger={6}",

  // --- sm_zone_export ---
  "Zone Export Done": "Exported {1} zone(s) to {2}.",

  // --- sm_zone_import ---
  "Zone Import No File": "No zones file for {1}.",
  "Zone Import Bad Json": "Zones file is not valid JSON.",
  "Zone Import Done": "Imported {1} zone(s).",
  "Zone Import Error": "Import error: {1}",

  // --- sm_zone_show ---
  "Usage Zone Show": "Usage: sm_zone_show <name|all> [seconds] (default 30; 0 = persistent)",
  "Zone Show All Timed": "Showing {1} zone(s) for {2}s.",
  "Zone Show All Persistent": "Showing {1} zone(s) (persistent).",
  "Zone Show One Timed": "Showing '{1}' for {2}s.",
  "Zone Show One Persistent": "Showing '{1}' (persistent).",

  // --- sm_zone_hide ---
  "Zone Hide All Done": "Hid {1} zone(s).",
  "Zone Not Shown": "Zone '{1}' is not shown.",
  "Zone Hide One Done": "Hid '{1}'.",
};
