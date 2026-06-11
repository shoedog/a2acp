FROM docker.io/library/debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends tinyproxy curl \
    && rm -rf /var/lib/apt/lists/*
COPY tinyproxy.conf /etc/tinyproxy/tinyproxy.conf
COPY tinyproxy.filter /etc/tinyproxy/filter
EXPOSE 8888
CMD ["tinyproxy", "-d", "-c", "/etc/tinyproxy/tinyproxy.conf"]
