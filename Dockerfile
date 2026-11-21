# Builds a static fritz binary and runs it on Alpine.
FROM rust:1.100-alpine AS builder

# ring needs a C compiler and the musl headers.
RUN apk add --no-cache musl-dev

WORKDIR /src
COPY . .
RUN cargo build --release --locked -p fritz \
    && cp target/release/fritz /usr/local/bin/fritz


FROM alpine:3.24

LABEL com.docker.desktop.mcp="true"
LABEL com.docker.desktop.mcp.name="fritz"

# apk upgrade pulls fixes for Alpine packages released after the base image.
RUN apk upgrade --no-cache && adduser -D -H -u 10001 fritz
COPY --from=builder /usr/local/bin/fritz /usr/local/bin/fritz
USER 10001

# A container has no keychain: the password comes from FRITZ_PASSWORD, the
# device from FRITZ_HOST, FRITZ_USERNAME and FRITZ_TLS_FINGERPRINT.
ENV FRITZ_NO_KEYCHAIN=1

# `docker run udondan/fritz <command>` runs the CLI; without a command the
# image starts the MCP server, which is what the Docker MCP Toolkit expects.
ENTRYPOINT ["fritz"]
CMD ["mcp"]
