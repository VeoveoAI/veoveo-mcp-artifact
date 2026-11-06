// Artifact's library: pages of the artifacts a person may read, newest first, each opening through
// the Hub's /view/ page, which frames it on the content domain.
veoveoGallery((ui) => {
  const address = (search, cursor) => {
    const query = new URLSearchParams();
    if (search) query.set("name", search);
    if (cursor) query.set("cursor", cursor);
    const text = query.toString().replaceAll("+", "%20");
    return text ? `artifact://artifacts?${text}` : "artifact://artifacts";
  };
  return {
    title: "Artifacts",
    placeholder: "Name starts with",
    remoteSearch: true,
    refresh: true,
    list: async ({ search, cursor }) => {
      const page = await ui.read(address(search, cursor));
      const items = [];
      for (const { artifact, level } of page.items) {
        items.push({
          key: artifact.artifactId,
          name: artifact.name,
          kind: ui.kindOf(artifact.name, artifact.mediaType),
          about: ui.size(artifact.size),
          fields: { ...artifact, level },
          url: `/view/artifact/${artifact.artifactId}/${encodeURIComponent(artifact.name)}`,
        });
      }
      return { items, next: page.nextCursor, empty: "No artifacts here." };
    },
    thumbnail: (item, size) => ui.tool("thumbnail", { artifactId: item.key, size }, ui.PICTURE_TIMEOUT),
    thumbnails: async (items, size) => (await ui.tool("thumbnails", { artifactIds: items.map((item) => item.key), size }, ui.PICTURE_TIMEOUT)).thumbnails,
    open: (item) => item.url,
  };
});
