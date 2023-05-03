const WindowForegroundListener = require("./index");

const windowForegroundListener = new WindowForegroundListener();

windowForegroundListener.start(0, (name) => {
  console.log(name);
});