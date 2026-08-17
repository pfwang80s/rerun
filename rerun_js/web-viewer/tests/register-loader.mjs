import { register } from "node:module";

process.env.RERUN_WEB_VIEWER_TEST = "1";
register("./loader.mjs", import.meta.url);
