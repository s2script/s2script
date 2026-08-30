import type { Recipe } from "../recipe.ts";
import * as admin from "./admin.ts";
import * as chat from "./chat.ts";
import * as clients from "./clients.ts";
import * as config from "./config.ts";
import * as consoleRecipe from "./console.ts";
import * as contracts from "./contracts.ts";
import * as cookies from "./cookies.ts";
import * as damage from "./damage.ts";
import * as db from "./db.ts";
import * as events from "./events.ts";
import * as gamerules from "./gamerules.ts";
import * as http from "./http.ts";
import * as items from "./items.ts";
import * as menu from "./menu.ts";
import * as movement from "./movement.ts";
import * as net from "./net.ts";
import * as playerState from "./player-state.ts";
import * as server from "./server.ts";
import * as sound from "./sound.ts";
import * as team from "./team.ts";
import * as timers from "./timers.ts";
import * as trace from "./trace.ts";
import * as translations from "./translations.ts";
import * as unsafe from "./unsafe.ts";
import * as transmit from "./transmit.ts";
import * as usercmd from "./usercmd.ts";
import * as usermessages from "./usermessages.ts";
import * as voice from "./voice.ts";
import * as ws from "./ws.ts";
import * as zones from "./zones.ts";

/** Every recipe the cookbook fans out. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  admin,
  chat,
  clients,
  config,
  consoleRecipe,
  contracts,
  cookies,
  damage,
  db,
  events,
  gamerules,
  http,
  items,
  menu,
  movement,
  net,
  playerState,
  server,
  sound,
  team,
  timers,
  trace,
  translations,
  transmit,
  unsafe,
  usercmd,
  usermessages,
  voice,
  ws,
  zones,
];
