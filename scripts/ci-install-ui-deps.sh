#!/usr/bin/env bash
# Install the system libraries egui/winit need on the Linux CI runners.
# apt mirrors (security.ubuntu.com in particular) sometimes fail to resolve on
# hosted runners, so retry the whole update+install a few times with backoff.
set -euo pipefail

packages=(
  pkg-config libxkbcommon-dev libwayland-dev libx11-dev
  libxcursor-dev libxrandr-dev libxi-dev
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
  libgl1-mesa-dev
)
apt_opts=(-o Acquire::Retries=5 -o Acquire::http::Timeout=30)
delays=(10 30)

for attempt in 1 2 3; do
  extra=()
  if [ "$attempt" -eq 3 ]; then
    extra=(--fix-missing)
  fi
  if sudo apt-get "${apt_opts[@]}" update \
    && sudo apt-get "${apt_opts[@]}" install -y --no-install-recommends "${extra[@]}" "${packages[@]}"; then
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
