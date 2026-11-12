# Image du relais Distant : le binaire statique, et rien d'autre.
#
# Construite par le workflow de release, qui compile d'abord le relais pour
# `x86_64-unknown-linux-musl` et le place à côté de ce fichier :
#
#   docker build -t open-remote-relay .
#
# Le port n'est à publier qu'en local (127.0.0.1:21116:21116) quand un serveur
# web tient le port 443 devant le relais.
FROM scratch
COPY --chmod=755 distant-relay /distant-relay
# Utilisateur sans droits (« nobody ») : le relais n'écrit nulle part.
USER 65534:65534
EXPOSE 21116
ENTRYPOINT ["/distant-relay"]
CMD ["--listen", "0.0.0.0:21116"]
