const fakeViewerModule = new URL("./support/re_viewer.mjs", import.meta.url).href;

export async function resolve(specifier, context, nextResolve) {
  if (
    specifier === "./re_viewer" &&
    context.parentURL?.endsWith("/rerun_js/web-viewer/index.js")
  ) {
    return {
      shortCircuit: true,
      url: fakeViewerModule,
    };
  }

  return nextResolve(specifier, context);
}
