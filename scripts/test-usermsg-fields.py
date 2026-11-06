#!/usr/bin/env python3
"""Exercise the production usermessage field accessors against real protobuf reflection."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
text = (ROOT / "shim/src/s2script_mm.cpp").read_text()
walk = text[text.index("static const google::protobuf::Message* s2_usermsg_walk("):
            text.index("// Subscribe-time validation")]
fields = text[text.index("static int s2_usermsg_hook_read_strings("):
              text.index("static int s2_usermsg_hook_has_field(")]
fixture = r'''
#include <cassert>
#include <cstring>
#include <limits>
#include <string>
#include <google/protobuf/descriptor.pb.h>
#include <google/protobuf/dynamic_message.h>
#include "json.hpp"
static google::protobuf::Message* s_hookMsg = nullptr;
'''
checks = r'''
int main() {
    using namespace google::protobuf;
    FileDescriptorProto file; file.set_name("fields.proto");
    auto* type = file.add_message_type(); type->set_name("Fields");
    auto add = [&](const char* name, FieldDescriptorProto::Type kind, bool repeated = false) {
        auto* field = type->add_field(); field->set_name(name);
        field->set_number(type->field_size()); field->set_type(kind);
        field->set_label(repeated ? FieldDescriptorProto::LABEL_REPEATED : FieldDescriptorProto::LABEL_OPTIONAL);
        return field;
    };
    add("params", FieldDescriptorProto::TYPE_STRING, true);
    add("signed_value", FieldDescriptorProto::TYPE_INT32);
    add("unsigned_value", FieldDescriptorProto::TYPE_UINT32);
    add("fixed_value", FieldDescriptorProto::TYPE_FIXED32);
    add("text", FieldDescriptorProto::TYPE_STRING);
    add("numbers", FieldDescriptorProto::TYPE_INT32, true);
    add("child", FieldDescriptorProto::TYPE_MESSAGE)->set_type_name("Fields");
    DescriptorPool pool; const auto* built = pool.BuildFile(file); assert(built);
    DynamicMessageFactory factory; auto* message = factory.GetPrototype(built->message_type(0))->New();
    const auto* d = message->GetDescriptor(); const auto* r = message->GetReflection();
    s_hookMsg = message;
    char empty[3]; assert(s2_usermsg_hook_read_strings("params", empty, sizeof(empty)) == 2);
    assert(std::string(empty) == "[]");
    const std::string long_text = std::string(5000, 'x') + u8"🧹 Decluttr";
    r->AddString(message, d->FindFieldByName("params"), "#Notification");
    r->AddString(message, d->FindFieldByName("params"), long_text);
    int length = s2_usermsg_hook_read_strings("params", nullptr, 0); assert(length > 5000);
    std::string out(length + 1, '\0');
    assert(s2_usermsg_hook_read_strings("params", out.data(), out.size()) == length);
    const auto values = nlohmann::json::parse(out.c_str());
    assert(values.size() == 2 && values[0] == "#Notification" && values[1] == long_text);
    assert(s2_usermsg_hook_read_strings("params", empty, sizeof(empty)) == -1);
    for (const char* invalid : {"missing", "text", "numbers", "params.child"})
        assert(s2_usermsg_hook_read_strings(invalid, nullptr, 0) == -1);
    assert(s2_usermsg_hook_set_int("signed_value", -2147483648LL) == 1);
    assert(s2_usermsg_hook_set_int("unsigned_value", 4294967295LL) == 1);
    assert(s2_usermsg_hook_set_int("fixed_value", 4294967295LL) == 1);
    assert(r->GetInt32(*message, d->FindFieldByName("signed_value")) == -2147483648LL);
    assert(r->GetUInt32(*message, d->FindFieldByName("unsigned_value")) == 4294967295ULL);
    assert(r->GetUInt32(*message, d->FindFieldByName("fixed_value")) == 4294967295ULL);
    assert(s2_usermsg_hook_set_int("signed_value", 2147483648LL) == 0);
    assert(s2_usermsg_hook_set_int("unsigned_value", -1) == 0);
    assert(s2_usermsg_hook_set_int("unsigned_value", 4294967296LL) == 0);
    for (const char* invalid : {"missing", "text", "numbers", "params", "child.signed_value"})
        assert(s2_usermsg_hook_set_int(invalid, 1) == 0);
    auto* child = r->MutableMessage(message, d->FindFieldByName("child"));
    assert(s2_usermsg_hook_set_int("child.signed_value", 42) == 1);
    assert(child->GetReflection()->GetInt32(*child, d->FindFieldByName("signed_value")) == 42);
    assert(r->GetRepeatedString(*message, d->FindFieldByName("params"), 1) == long_text);
    assert(r->GetUInt32(*message, d->FindFieldByName("unsigned_value")) == 4294967295ULL);
    s_hookMsg = nullptr;
    assert(s2_usermsg_hook_read_strings("params", nullptr, 0) == -1);
    assert(s2_usermsg_hook_set_int("signed_value", 1) == 0);
    delete message;
}
'''
with tempfile.TemporaryDirectory(prefix="s2script-usermsg-") as directory:
    cpp, binary = Path(directory) / "fields.cpp", Path(directory) / "fields"
    cpp.write_text(fixture + walk + fields + checks)
    subprocess.run([os.environ.get("CXX", "g++"), "-std=c++17", "-D_GLIBCXX_USE_CXX11_ABI=0",
                    "-fsanitize=address,undefined", "-fno-sanitize-recover=all",
                    "-I", str(ROOT / "third_party/hl2sdk/thirdparty/protobuf-3.21.8/src"),
                    "-I", str(ROOT / "shim/third_party"), str(cpp),
                    str(ROOT / "third_party/hl2sdk/lib/linux64/release/libprotobuf.a"),
                    "-pthread", "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True, timeout=15)
print("production usermessage fields: passed")
