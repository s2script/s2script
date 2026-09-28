// fixture proves package boundary only; it is not a supported live game.
const twice = __s2_package_function('twice');
({ '.': { twice(value) { return twice.call(value); } } });
