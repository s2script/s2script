import type { Recipe } from "../recipe.ts";
import { httpRecipe } from "./http.ts";
import { soundRecipe } from "./sound.ts";
import { wsRecipe } from "./ws.ts";

/** Every recipe the cookbook registers. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  httpRecipe,
  soundRecipe,
  wsRecipe,
];
