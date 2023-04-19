#!/bin/bash
# This script helps you run the mobilecoind buddy inside the container alongside mobilecoind if you prefer to do that.
# After starting the container you also need to run:
# sudo apt-get update && sudo apt-get install -y libxcursor-dev libxrandr-dev libxi-dev libx11-xcb-dev libgl1-mesa-glx libgl1-mesa-dev

# Set variables for customization
PATH_ON_HOST="/path/on/host"

# Run Docker container with privileged access and port bindings
docker run --privileged \
-e DISPLAY=$DISPLAY \
-v /tmp/.X11-unix:/tmp/.X11-unix \
--env=ENTRYPOINT_VERBOSE=1 \
--add-host=host:$(hostname -I | awk '{print $1}') \
--volume=$(pwd):/tmp/mobilenode \
--workdir=/tmp/mobilenode \
--env=EXTERNAL_UID=$(id -u) \
--env=EXTERNAL_GID=$(id -g) \
--env=EXTERNAL_USER=$(id -un) \
--env=EXTERNAL_GROUP=$(id -gn) \
--env=CARGO_TARGET_DIR=/tmp/mobilenode/target/docker \
--env=MC_CHAIN_ID=local \
--env=TEST_DATABASE_URL=postgres://localhost \
--env=RUST_BACKTRACE=1 \
--env=CARGO_BUILD_JOBS=8 \
--env=SGX_MODE=SW \
--env=IAS_MODE=DEV \
--env=GIT_COMMIT=f453a2458 \
--cap-add=SYS_PTRACE \
-ti \
--publish 8080:8080 \
--publish 8081:8081 \
--publish 8443:8443 \
--publish 3223:3223 \
--publish 3225:3225 \
--publish 3226:3226 \
--publish 3228:3228 \
--publish 4444:4444 \
--env=SSH_AUTH_SOCK \
--volume=$SSH_AUTH_SOCK:$SSH_AUTH_SOCK \
--volume=$HOME/.ssh:/var/tmp/user/.ssh \
--volume=$PATH_ON_HOST:/tmp/testnet \
mobilecoin/builder-install:v0.0.25 \
/bin/bash

