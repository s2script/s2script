#pragma once

// Config path resolver and engine-op implementations. Returned pointers refer to module-owned
// buffers and remain valid until the next call to the same operation.
const char* s2_config_path_resolve(const char* id);
const char* s2_config_read(const char* id);
int s2_config_write(const char* id, const char* content);
const char* s2_config_read_file(const char* name);
int s2_config_write_file(const char* name, const char* content);
