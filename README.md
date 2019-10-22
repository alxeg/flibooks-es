# flibooks-es
Flibusta's backups books search client (ElasticSearch based)


For debug purpose you can start es locally via docker, e.g
```bash
docker run -d -p 9200:9200 -p 9300:9300 -v `pwd`/data:/usr/share/elasticsearch/data -e "discovery.type=single-node" --restart=always docker.elastic.co/elasticsearch/elasticsearch:7.4.0
```