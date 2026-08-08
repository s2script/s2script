/** basebans — English seed. Generated into translations/basebans.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  "Usage Ban": "{green}[SM]{default} Usage: sm_ban <#userid|name> <minutes> [reason]",
  "Ban Ambiguous Target": "{green}[SM]{default} '{1}' matches multiple players; be more specific.",
  "Cannot Ban No Steamid": "{green}[SM]{default} Cannot ban {1} (no SteamID — bot or unauthenticated).",
  "Ban Duration Permanently": " permanently",
  "Ban Duration Minute": " for {1} minute",
  "Ban Duration Minutes": " for {1} minutes",
  "Ban Reason Suffix": " ({1})",
  "Ban Success": "{green}[SM]{default} Banned {1}{2}{3}.",

  "Usage Unban": "{green}[SM]{default} Usage: sm_unban <steamid64>",
  "Unban Success": "{green}[SM]{default} Unbanned {1}.",
  "Unban Not Banned": "{green}[SM]{default} {1} was not banned.",

  "Usage Addban": "{green}[SM]{default} Usage: sm_addban <steamid64> <minutes> [reason]",
  "Addban Duration Minutes": " ({1} min)",
  "Addban Duration Permanent": " (permanent)",
  "Addban Reason Suffix": " {1}",
  "Addban Success": "{green}[SM]{default} Added ban for {1}{2}{3}.",

  // The four keys below feed the banned player's kick reason (chat + console via
  // Client.kickWithReason -> Client.chat/Client.print), which bypasses the colour-expanding
  // chat/console funnels entirely — no {tag} in any of these, or it would render as literal text.
  "Ban Message": "[SM] You are banned: {1} ({2})",
  "Ban Reason Default": "No reason",
  "Ban Expiry Permanent": "permanent",
  "Ban Expiry Minutes": "expires in {1} min",
  "Ban Reason By Admin": "Banned by admin",
  // Fallback engine kick reason (no live Client wrapper) — same no-colour rule as above.
  "Kick Ban Reason Fallback": "Banned: {1}",
  // adminmenu "Kick" proof item's kick reason — also an engine kick reason, no colour.
  "Kick By Admin": "Kicked by admin",

  // adminmenu ban-menu bot/unauthenticated notice — sent via Client.chat directly (not Chat.toSlot
  // or cmd.reply), so this one is colour-free too.
  "Cannot Ban Bot": "Cannot ban {1} (bot / not authenticated)",

  // adminmenu ban-duration sub-menu (Menu title + item labels). Also colour-free, for a different
  // reason than the kick-reason phrases above: this menu is style=Center, and the Center renderer
  // (games/cs2/js/pawn.js renderHtml) HTML-escapes title/item text and paints its OWN fixed
  // <font color> styling — it never calls __s2_colors.expand, so a {tag} here would render as
  // literal, escaped "{green}" text in the menu, not a colour.
  "Ban Menu Title": "Ban {1} for",
  "Ban Menu Permanent": "Permanent",
  "Ban Menu Minutes": "{1} min",

  // adminmenu topmenu item names — MenuStyle.Center, resolved once at registration (no viewer
  // yet), same limitation/pattern as basecommands' "Change Map Item".
  "Kick Item": "Kick",
  "Ban Item": "Ban",
};
