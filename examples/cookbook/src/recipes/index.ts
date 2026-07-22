import type { Recipe } from "../recipe.ts";
import { cookiesRecipe } from "./cookies.ts";
import { eventsRecipe } from "./events.ts";
import { gamerulesRecipe } from "./gamerules.ts";
import { httpRecipe } from "./http.ts";
import { menuRecipe } from "./menu.ts";
import { netRecipe } from "./net.ts";
import { playerStateRecipe } from "./player-state.ts";
import { serverRecipe } from "./server.ts";
import { soundRecipe } from "./sound.ts";
import { traceRecipe } from "./trace.ts";
import { translationsRecipe } from "./translations.ts";
import { transmitRecipe } from "./transmit.ts";
import { usercmdRecipe } from "./usercmd.ts";
import { usermessagesRecipe } from "./usermessages.ts";
import { wsRecipe } from "./ws.ts";

/** Every recipe the cookbook registers. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  cookiesRecipe,
  eventsRecipe,
  gamerulesRecipe,
  httpRecipe,
  menuRecipe,
  netRecipe,
  playerStateRecipe,
  serverRecipe,
  soundRecipe,
  traceRecipe,
  translationsRecipe,
  transmitRecipe,
  usercmdRecipe,
  usermessagesRecipe,
  wsRecipe,
];
