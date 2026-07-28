/** The @fixture/ik-mapchooser contract — published by the differently-named @fixture/ik-mce
 *  PACKAGE, which is the whole point of this fixture (design spec §5.3.0). Deliberately the same
 *  SHAPE as the consumer's registry copy of @fixture/ik-mce, so that resolving the wrong one still
 *  typechecks green and the only visible difference is the hash. */
export interface Api {
  nominate(map: string): void;
}
