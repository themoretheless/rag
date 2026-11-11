#!/usr/bin/env bash
# Install the system libraries egui/winit need on the Linux CI runners.
#
# Hosted runners sometimes cannot resolve security.ubuntu.com for long
# stretches, which made apt hang or fail before any build ran. The runner's
# archive mirror also serves the -security pocket, so point security sources
# at it (same package versions as the installed base libraries), then retry a
# bounded update+install a few times.
set -euo pipefail

mirror="${CI_APT_MIRROR:-azure.archive.ubuntu.com}"
packages=(
  pkg-config libxkbcommon-dev libwayland-dev libx11-dev
  libxcursor-dev libxrandr-dev libxi-dev
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
  libgl1-mesa-dev
)
apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20)
delays=(10 30)

# Rewrite security.ubuntu.com in classic .list and deb822 .sources files.
rewrite_security_sources() {
  local files=()
  [ -f /etc/apt/sources.list ] && files+=(/etc/apt/sources.list)
  shopt -s nullglob
  files+=(/etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources)
  shopt -u nullglob
  if [ "${#files[@]}" -gt 0 ]; then
    sudo sed -i "s#://security\.ubuntu\.com/#://${mirror}/#g" "${files[@]}"
  fi
}

rewrite_security_sources

for attempt in 1 2 3; do
  extra=()
  if [ "$attempt" -eq 3 ]; then
    extra=(--fix-missing)
  fi
  if sudo timeout 300 apt-get "${apt_opts[@]}" update \
    && sudo timeout 300 apt-get "${apt_opts[@]}" install -y --no-install-recommends "${extra[@]}" "${packages[@]}"; then
    exit 0
  fi
  if [ "$attempt" -lt 3 ]; then
    delay=${delays[$((attempt - 1))]}
    echo "apt attempt $attempt failed; retrying in ${delay}s" >&2
    sleep "$delay"
  fi
done
echo "apt failed after 3 attempts" >&2
exit 1
