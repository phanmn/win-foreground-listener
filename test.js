const WindowForegroundListener = require("./index");

const windowForegroundListener = new WindowForegroundListener();

windowForegroundListener.start(27108, (name) => {
  console.log(name);
});