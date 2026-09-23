// 代理生命周期：启动、停止、状态查询。
// 监听固定 127.0.0.1；端口占用时直接报错，不静默顺延（界面据此提示改端口）。

'use strict';

const { createProxyServer } = require('./server');

function createLocalProxy({ getConfig, onLog, maxBodyBytes }) {
  let server = null;
  // 进行中的启动：并发调用复用同一次尝试。不加这个，两个 start() 会各自 listen，
  // 后一个失败时把共享的 server 置空，反而抹掉前一个已经监听成功的引用
  let starting = null;
  // 进行中的停止：它会认领在途的那次启动并把它关掉，所以这期间到达的 start() 不能复用
  // starting（那是一次正在被拆除的尝试，复用它就会 resolve 出一个实际没在监听的端口），
  // 必须等停止收完尾再新起一次监听
  let stopping = null;

  function beginListen() {
    starting = new Promise((resolve, reject) => {
      const srv = createProxyServer({ getConfig, onLog, maxBodyBytes });
      const port = getConfig().settings.port;
      srv.once('error', (err) => {
        starting = null;
        reject(err);
      });
      srv.listen(port, '127.0.0.1', () => {
        server = srv;
        starting = null;
        resolve({ port: srv.address().port });
      });
    });
    return starting;
  }

  function closeServer() {
    return new Promise((resolve) => {
      server.close(() => {
        server = null;
        resolve();
      });
    });
  }

  function start() {
    // 必须先于 server 判断：收尾中的那个监听还在（关闭回调未落地），复用它会拿到一个
    // 即将被关掉的端口。等收尾结束，再按下面的正常路径起一次新的
    if (stopping) {
      return stopping.then(start);
    }
    if (server) {
      return Promise.resolve({ port: server.address().port });
    }
    if (starting) {
      return starting;
    }
    return beginListen();
  }

  function stop() {
    if (stopping) {
      return stopping;
    }
    const teardown = (async () => {
      // 在途的 start() 还没把 server 填上，直接返回会把随后落地的监听留在运行态：
      // 先等那次尝试有结果再关。启动失败的那次没有监听要关，吞掉即可——
      // stop() 的契约是「让它停下来」，不该因为一次启动失败而失败
      if (starting) {
        try {
          await starting;
        } catch (error) {
          // 在途启动失败，没有监听需要关
        }
      }
      if (server) await closeServer();
    })();
    // 摘句柄要挂在 teardown 后面，不能写在上面那个 async 体内：无需等待时那个体会同步
    // 跑完，句柄会在 `stopping = ...` 赋值之前就被清掉，留下一个永不失效的 stopping
    stopping = teardown.finally(() => {
      stopping = null;
    });
    return stopping;
  }

  return {
    start,
    stop,
    getStatus() {
      return {
        running: !!server,
        port: server ? server.address().port : (getConfig().settings.port || null)
      };
    }
  };
}

module.exports = { createLocalProxy };
