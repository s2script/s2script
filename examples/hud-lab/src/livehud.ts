/**
 * Descriptor for `s2script_hud_live.xml` — the production layout in workshop addon 3790153369.
 *
 * WHY A SEPARATE DESCRIPTOR: the shipped default (`s2script_hud.xml`) is a literal-text PROBE —
 * it renders its own words with no server involvement, which is what makes it useful for proving
 * the pipeline. This one is the opposite: every slot is a `{s:...}` binding, so it renders as an
 * EMPTY FRAME until the server fills it. A blank panel here is an unfilled panel, not a broken one.
 *
 * It follows the `id == varName` convention throughout, so `layout.forSlot(slot).set(id, value)` is
 * the whole binding and `text` below is filled programmatically rather than hand-mapped.
 */
import type { CustomHudSpec } from "@s2script/cs2";

/** Every text-bearing id in the layout. id === dialog variable name. */
const TEXT_IDS = [
  "timer_value", "timer_label",
  "feed_0_a", "feed_0_w", "feed_0_v", "feed_0_t",
  "feed_1_a", "feed_1_w", "feed_1_v", "feed_1_t",
  "feed_2_a", "feed_2_w", "feed_2_v", "feed_2_t",
  "feed_3_a", "feed_3_w", "feed_3_v", "feed_3_t",
  "team_ct_name", "team_ct_score", "team_t_name", "team_t_score",
  "pcard_name", "pcard_meta", "pcard_badge_t",
  "pcard_k", "pcard_d", "pcard_a", "pcard_hs", "pcard_form_label",
  "vote_q", "vote_sub", "vote_yes", "vote_no",
  "motd_title", "motd_sub", "motd_h0", "motd_p0", "motd_h1", "motd_p1",
  "motd_h2", "motd_p2", "motd_note", "motd_ok_t",
] as const;

/** id === varName, so the map is its own identity. Built rather than typed out twice. */
const TEXT: Record<string, string> = {};
for (const id of TEXT_IDS) TEXT[id] = id;

export const LIVE_HUD: CustomHudSpec = {
  addons: ["3790153369"],
  // `.xml` SOURCE extension — the client rejects `.vxml` / `.vxml_c`.
  resource: "panorama/layout/custom_game/s2script_hud_live.xml",
  // `s2-hide` is `visibility: collapse` — it removes the panel from flow entirely, which is what
  // the layout uses for unused feed rows and the MOTD layer.
  hideClass: "s2-hide",
  text: TEXT,
  buttons: ["motd_ok"],
  // Width is a step class family, never a number — the framework rounds into it.
  meters: { timer: "timer_fill", vote: "vote_fill" },
};

/** Panels that start collapsed and must be revealed explicitly. */
export const LIVE_PANELS = {
  motd: "motd_layer",
  vote: "vote",
  feed: ["feed_0", "feed_1", "feed_2", "feed_3"] as const,
} as const;
