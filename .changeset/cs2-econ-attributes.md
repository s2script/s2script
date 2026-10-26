---
"@s2script/cs2": minor
---

Set econ attributes on weapons. `weapon.setAttribute(name, value)` sets or adds an attribute by its
definition name ("set item texture prefab", "set item texture wear", "kill eater") on the item's
networked dynamic attribute list, through `CAttributeList::SetOrAddAttributeValueByName`, the same
function ModSharp and Swiftly call.
