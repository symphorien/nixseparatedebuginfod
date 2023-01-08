create table if not exists builds (
  buildid text unique not null,
  executable text,
  debuginfo text,
  source text
  );

create index if not exists bybuildid on builds(buildid);

create table if not exists version (version int not null);

create table if not exists timestamps (
  gc int not null,
  storepath int not null
);

