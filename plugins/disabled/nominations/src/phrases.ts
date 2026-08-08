/**
 * nominations — English seed. Generated into translations/nominations.phrases.json by
 * scripts/gen-phrases.mjs.
 *
 * Colour palette carried over from the original ChatColors constants (TAG/MAP/WHO/NUM/BAD/BODY):
 * green [Nominate] tag, grey body, lime map names, olive who-nominated, yellow "type this word",
 * lightred refusals. All routed through Chat.toSlot/Chat.toAll (chat funnel, expands tags) or the
 * MenuStyle.Chat renderer (also Chat.toSlot under the hood — see mapMenu in plugin.ts), so every
 * tag below renders exactly as the original control byte did.
 */
export const phrases = {
  "Nominate Is Current Map": "{green}[Nominate] {grey}{lime}{1}{grey} is {lightred}the current map{grey}.",
  "Nominate Played Too Recently": "{green}[Nominate] {grey}{lime}{1}{grey} was {lightred}played too recently{grey}.",
  "Nominate Already Nominated": "{green}[Nominate] {grey}{lime}{1}{grey} is {lightred}already nominated{grey}.",

  // Broadcast (Chat.toAll) — everyone sees who nominated what, so this resolves at the server
  // default language, not the nominator's own.
  "Nominate Announced": "{green}[Nominate] {grey}{olive}{1}{grey} nominated {lime}{2}{grey}.",

  "Nominate Menu Closed": "{green}[Nominate] {grey}Closed — type {yellow}nominate{grey} to open it again.",
  "Nominate Menu Timed Out": "{green}[Nominate] {grey}Timed out — type {yellow}nominate{grey} to open it again.",
  "Nominate None Available": "{green}[Nominate] {lightred}No maps available to nominate right now.",

  // Menu titles/items — MenuStyle.Chat, so these also go through the tag-expanding Chat.toSlot path.
  "Nominate Menu Title": "Nominate a map",
  "Nominate Disambiguation Title": "Did you mean...",
  "Nominate Recent Item": "{grey}{1} {lightred}- (too recently played)",
  "Nominate Available Item": "{lime}{1}",

  // sm_nominate command replies — no colour/"[SM]" prefix in the original.
  "Must Be In Game": "Nominate in-game.",
  "No Map Matching": "No map matching '{1}'.",
};
