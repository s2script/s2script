/**
 * rockthevote — English seed. Generated into translations/rockthevote.phrases.json by
 * scripts/gen-phrases.mjs.
 *
 * Colour palette carried over from the original ChatColors constants (TAG/MAP/NUM/BAD/BODY): green
 * [RTV] tag, grey body, lime map names, yellow "act on this number", lightred refusals — all routed
 * through Chat.toAll/Chat.toSlot (chat funnel, expands tags), so every tag below renders exactly as
 * the original control byte did.
 *
 * NOT converted, deliberately:
 *  - The live-tally HUD's `<font color='#xxxxxx'>` values (Vote.registerTallyRenderer's `show`) are
 *    raw hex colours in a `show_survival_respawn_status` HTML payload — a completely different
 *    rendering path from the chat control-byte system this slice's {tag} mechanism covers (it never
 *    reaches __s2_colors.expand). Only the HUD's static English WORDING moved to phrases below; the
 *    hex colours stay literal in plugin.ts.
 *  - `DONT_CHANGE` ("Don't Change") and the vote `options` array entries generally: these are
 *    structural sentinels compared by strict equality (finishVote's `chosen === DONT_CHANGE`) and
 *    fed to the engine-generic Vote primitive, which has no translation hook for options at all —
 *    same treatment as basevotes'/funvotes' untranslated "Yes"/"No". Where "Don't Change" appears
 *    in a DISPLAYED sentence it is re-typed as static phrase text below (never substituted from the
 *    sentinel), so nothing here depends on translating the sentinel itself.
 *  - `Vote.start({ question: "RockTheVote", ... })` — matched verbatim by
 *    `UserMessages.onPre`'s debugString check that suppresses the core's own ballot line; translating
 *    it would break that match.
 */
export const phrases = {
  "Rtv Ballot Header": "{green}[RTV] {grey}Vote for the next map — type its {yellow}number{grey} in chat:",
  "Ballot Option Dont Change": "  {yellow}{1}{grey}. {grey}{2}",
  "Ballot Option Map": "  {yellow}{1}{grey}. {lime}{2}",

  "Rtv Vote Tied": "{green}[RTV] {lightred}Vote tied{grey} — the map stays.",
  "Rtv Dont Change Won": "{green}[RTV] {lightred}Don't Change{grey} won — the map stays.",
  "Rtv Winner Invalid": "{green}[RTV] {lightred}Winner invalid{grey} — the map is unchanged.",
  "Rtv Winner": "{green}[RTV] {lime}{1}{grey} won — changing at the end of the round.",
  "Rtv No Maps Available": "{green}[RTV] {lightred}No maps available to vote on.",

  // requestRtv — Chat.toSlot to the requesting player.
  "Rtv Vote Already Running": "{green}[RTV] {grey}A vote is already running.",
  "Rtv Vote Already Happened": "{green}[RTV] {grey}A vote already happened this map.",
  "Rtv Not Open Yet": "{green}[RTV] {grey}RockTheVote is not open yet — {yellow}{1}s{grey} to go.",
  "Rtv Already Voted": "{green}[RTV] {grey}You already voted to rock the vote — {yellow}{1}{grey} needed in total.",
  "Rtv Not Enough Players": "{green}[RTV] {lightred}Not enough players.",
  // Broadcast (Chat.toAll) — everyone sees this tally update, so it resolves at the server default
  // language, not the requesting player's.
  "Rtv Wants To Vote": "{green}[RTV] {lime}{1}{grey} wants to rock the vote — {yellow}{2}{grey} more needed.",

  // sm_forcertv's cmd.reply — bare strings, no TAG/colour prefix in the original (unlike
  // requestRtv's colour-carrying "A vote is already running." above — same English text, genuinely
  // different rendering, so it is kept as its own colour-free key rather than reused).
  "Rtv Forced": "RTV forced.",
  "Rtv Forced Already Running": "A vote is already running.",

  // Live-tally HUD (show_survival_respawn_status HTML) — wording only; see the file-header note
  // above for why the hex colours around these stay in plugin.ts, untagged.
  // WHITESPACE IS SIGNIFICANT on the next two keys: "Rtv Hud Hint" is immediately followed in
  // plugin.ts by "{tally.secondsLeft}s" with no separator, and "Rtv Hud Left" immediately follows
  // that "s" the same way — a translation that trims the trailing/leading space will visibly run
  // the words together in the HUD ("chat5s left" / "5sleft"). Keep the space when translating.
  "Rtv Hud Title": "Rock The Vote",
  "Rtv Hud Hint": "type a number in chat · ",
  "Rtv Hud Left": " left",
};
