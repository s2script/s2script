#!/usr/bin/env bash
# Download the modules the offline checker and schema.py read, from one CS2 depot manifest, into a
# flat directory (anonymous Steam login; the dedicated server depot needs no account).
#
#   tools/gamedata-offline/fetch-depot.sh <manifest gid> <out dir>
#
# Needs DepotDownloader (https://github.com/SteamRE/DepotDownloader) on PATH or in $DEPOTDOWNLOADER.
set -euo pipefail

manifest=${1:?manifest gid}
out=${2:?out dir}
dd=${DEPOTDOWNLOADER:-DepotDownloader}
app=${CS2_APP:-730}
depot=${CS2_DEPOT:-2347773}

# Union of check.py's modules (PINS + VTABLE_MODULES) and schema.py's MODULES.
modules='libserver|libengine2|libtier0|libnetworksystem|libmatchmaking|libsoundsystem|libfilesystem_stdio'
modules+='|libanimationsystem|libmaterialsystem2|libmeshsystem|libparticles|libpulse_system'
modules+='|librendersystemempty|libresourcesystem|libscenefilecache|libscenesystem|libschemasystem'
modules+='|libvphysics2|libworldrenderer'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf 'regex:^game/(csgo/)?bin/linuxsteamrt64/(%s)\\.so$\n' "$modules" > "$work/filelist.txt"

attempt=0
until "$dd" -app "$app" -depot "$depot" -manifest "$manifest" -os linux -osarch 64 \
        -filelist "$work/filelist.txt" -dir "$work/depot"; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 3 ]; then
    echo "fetch-depot: DepotDownloader failed 3 times for manifest $manifest" >&2
    exit 1
  fi
  sleep $((attempt * 20))
done

mkdir -p "$out"
# game/bin first, then game/csgo/bin, so the game's own copy wins if a name is in both.
for d in "$work/depot/game/bin/linuxsteamrt64" "$work/depot/game/csgo/bin/linuxsteamrt64"; do
  [ -d "$d" ] && cp -f "$d"/*.so "$out"/ 2>/dev/null || true
done
missing=0
for m in ${modules//|/ }; do
  if [ ! -f "$out/$m.so" ]; then
    echo "fetch-depot: $m.so not in manifest $manifest" >&2
    missing=1
  fi
done
echo "$manifest" > "$out/.manifest"
ls -la "$out"
exit $missing
