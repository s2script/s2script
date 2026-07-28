/** The verified copy of the REGISTRY @fixture/ik-mce contract. A workspace sibling happens to
 *  carry that package name while publishing something else entirely, and it must not shadow this
 *  file (design spec §5.3.0, §11). Same shape as the sibling's contract on purpose — only the
 *  bytes differ, so a build that resolved the wrong one would still be green. */
export interface Api {
  nominate(map: string): void;
}
