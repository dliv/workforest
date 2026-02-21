CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  city TEXT,
  country TEXT,
  version TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  channel TEXT
);

CREATE INDEX idx_events_timestamp ON events(timestamp);
