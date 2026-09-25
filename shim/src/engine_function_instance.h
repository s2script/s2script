#pragma once
#include "engine_function_abi.h"
#include <map>
#include <set>
namespace s2bridge {
struct RecordField {
    std::string name, storage;
    uint32_t offset=0, width=0;
    bool nullable=false;
    uint8_t read=0, write=0;
};
struct RecordLayout {
    uint32_t extent=0, alignment=0;
    std::string hash, storage_key;
    std::vector<RecordField> fields;
};
struct RecordPosition { RecordLayout layout; bool nullable=false; };
struct InstancePositions {
    std::map<int,RecordPosition> records;
    std::set<int> hidden;
    bool nullable_receiver=false;
    bool restricted() const {return !records.empty() || !hidden.empty();}
    bool compatible(const InstancePositions& b) const {
        if(hidden!=b.hidden || records.size()!=b.records.size()) return false;
        for(const auto& row:records) {
            auto i=b.records.find(row.first);
            if(i==b.records.end() || i->second.layout.storage_key!=row.second.layout.storage_key) return false;
        }
        return true;
    }
};
}
