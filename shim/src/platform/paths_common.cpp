#include "paths.h"

namespace s2platform {

std::string AddonRootFromModulePath(const std::string& modulePath) {
    std::string path = modulePath;
    for (int i = 0; i < 3; ++i) {
        while (!path.empty() && (path.back() == '/' || path.back() == '\\')) path.pop_back();
        const size_t slash = path.find_last_of("/\\");
        if (slash == std::string::npos) return {};
        path.resize(slash);
    }
    return path;
}

}  // namespace s2platform
