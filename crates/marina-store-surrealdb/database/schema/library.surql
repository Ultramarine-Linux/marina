DEFINE TABLE library SCHEMAFULL;

DEFINE FIELD item_id ON TABLE library TYPE uuid;
DEFINE FIELD title ON TABLE library TYPE string;
DEFINE FIELD kind ON TABLE library TYPE string;
DEFINE FIELD platform_slug ON TABLE library TYPE option<string>;
DEFINE FIELD platform ON TABLE library TYPE record<platform>;
DEFINE FIELD provider_ids ON TABLE library TYPE object;
DEFINE FIELD summary ON TABLE library TYPE option<string>;
DEFINE FIELD alternative_names ON TABLE library TYPE option<array<string>>;
DEFINE FIELD tags ON TABLE library TYPE option<array<string>>;
DEFINE FIELD languages ON TABLE library TYPE option<array<string>>;
DEFINE FIELD regions ON TABLE library TYPE option<array<string>>;
DEFINE FIELD cover ON TABLE library TYPE option<string>;
DEFINE FIELD created_at ON TABLE library TYPE option<string>;
DEFINE FIELD released_at ON TABLE library TYPE option<string>;
DEFINE FIELD updated_at ON TABLE library TYPE option<string>;
DEFINE FIELD files ON TABLE library TYPE option<array<object>>;
DEFINE FIELD assets ON TABLE library TYPE option<array<object>>;
