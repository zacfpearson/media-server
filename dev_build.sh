sudo docker build -f docker/Dockerfile.dev -t media-server:dev docker
sudo docker run -it --rm --mount type=bind,source=$(pwd)/code,target=/build media-server:dev /bin/bash -c "cargo build"
sudo docker build --no-cache -f docker/Dockerfile.serve -t media-server:serve code
sudo docker run --name media-server -e ENDPOINT=mongodb://127.0.0.1:27017/media -e TABLE=violin --rm --network=host media-server:serve