import * as proto from './pb/main';
import { Empty } from './google/protobuf/empty';
import WebSocketAsPromised from 'websocket-as-promised';
import Channel from 'chnl';
  import { listenerCount } from 'stream';

export type APIError<Values> = {
  error?: string;
} & Partial<Values>;


export const vredditID = (url: string): string | undefined => {
  const res = /v\.redd\.it\/([A-Za-z0-9]+)/.exec(url);

  if (res?.length !== 2) {
    return undefined;
  }

  return res[1];
}

export const getVRedditFromComments = (url: string): Promise<string> => (
  new Promise((resolve, reject) => (
    // Thank you cors anywhere for this awesome hack.
    fetch(`//cors-anywhere.herokuapp.com/${url}`)
      .then((r) => r.text())
      .then((text) => {
        const res = /(https:\/\/v\.redd\.it\/[A-Z0-9a-z]+)/.exec(text);

        if (!res || res?.length < 2) {
          reject(new Error('Failed to find v.redd.it url in link; is it a video?'));
        } else {
          resolve(res[1]);
        }
      })
  ))
);

export const getURLsFromMPD = (xml: string): string[] => {
  const parser = new DOMParser();
  const xmlDoc = parser.parseFromString(xml, 'text/xml');

  return [].slice.call(
    xmlDoc.getElementsByTagName('BaseURL')
  ).map(({ nodeValue }) => nodeValue);
};

const getVRedditFromUser = (url: string, statusCb: (status: string) => void): Promise<string> => (
  new Promise((resolve, reject) => {
    if (/v\.redd\.it\/([A-Za-z0-9]+)/.test(url)) {
      statusCb("Scanning reddit");
      resolve(url);
    } else if (/(m|old)?\.?reddit\.com\/r\/.*?\/comments/.test(url)) {
      statusCb("Fetching thread");
      // comments link
      getVRedditFromComments(url)
        .then(resolve)
        .catch(reject);
    } else {
      statusCb("Failed");
      reject(new Error('Couldn\'t get a reddit link from that. Please email me@brod.es if this is a valid reddit link.'));
    }
  })
);

let connection: VRWebSocket | null = null;

const openConnection = (addr: string) => (
  new Promise<VRWebSocket>((resolve, reject) => {
    if (connection !== null) {
      resolve(connection);
    }
    connection = new VRWebSocket(addr);

    const timeout = setTimeout(() => {
      clearTimeout(timeout);
      if (connection !== null && !connection.isOpened) {
        console.log("timeout hit");
        console.dir(connection);
        connection.close();
        connection = null;
        reject("connection timeout");
      }
    }, 5000);

    return connection.open().then(() => {
      clearTimeout(timeout);
      resolve(connection!);
    }, reject);
  })
);

export type HostedURLOpts = {
  statusCallback: (status: string) => void;
  authz?: string;
  server?: string;
};

export const getHostedURL = (
  targetURL: string,
  opts: HostedURLOpts = { statusCallback: () => {} },
): Promise<string> => (
  // get a connection
  new Promise<VRWebSocket>((resolve, reject) => {
    const { authz, server, statusCallback } = opts;
    statusCallback("Connecting");

    if (server === undefined) {
      reject('Convert server was missing but is required');
    } else if (authz === undefined) {
      reject('Authorization was missing but is required');
    } else {
      return openConnection(server).then(resolve, reject);
    }
  })
  // handshake
).then((connection) => {
  opts.statusCallback("Checking permission");
  return connection
    .sendRequest(NewRequest.handshake(opts.authz))
    .then((resp) => {
      if (resp.$case === "accepted") {
        const accept = resp?.accepted;
        if (accept?.resultInner?.$case === "ok") {
          opts.statusCallback("Requesting");
          return connection;
        } else if (accept?.resultInner?.$case === "err") {
          opts.statusCallback("Server declined");
          throw new Error(`server rejected handshake: ${accept?.resultInner?.err}`);
        }
      } else {
        throw new Error(`unknown response from server after handshake: ${proto.TranscodeResp.toJSON({ resp })}`);
      }
      return connection;
    });
// download!
}).then((connection) => wsStartDownload(connection, targetURL, opts));

const wsStartDownload = (connection: VRWebSocket, targetURL: string, opts: HostedURLOpts) => new Promise<string>((rawResolve, rawReject) => {
  const { statusCallback } = opts;

  let gotStatus = false; // debounce
  let done = false;
  const resolve = (s: string) => {
    done = true;
    rawResolve(s);
  };
  const reject = (s: string) => {
    if (connection !== null && !done) {
      done = true;
    }
    rawReject(s);
  };

  const listen = (data: Body) => {
    if (done) {
      connection.onMessage.removeListener(listen);
    }
    console.log(`incoming message`, data);

    parseResponse(data).then((resp) => {
      console.log(`server message: ${resp}`);
      if (resp.$case !== "jobStatus") {
        statusCallback("Server communication failure");
        return reject(`unexpected message type, expected state: ${data}`);
      }
      const { state } = resp.jobStatus;

      if (!state) {
        statusCallback("Server communication failure");
        return reject(`unexpected message type, expected state: ${data}`);
      }
      const stateTy = state?.$case;

      if (stateTy === "queued") {
        statusCallback("Queueing");
        console.log("file queued");
      } else if (state?.$case === "completed") {
        const inner = state?.completed?.resultInner;
        switch (inner?.$case) {
          case "ok":
            console.log(`file COMPLETE: ${JSON.stringify(inner)}`);
            statusCallback("Completed");
            return resolve(inner.ok);
          case "err":
            statusCallback("Errored");
            console.log(`file failed: ${JSON.stringify(inner)}`);
            return reject(inner.err);
          default:
            return reject(`unknown result state for completed file from server: ${inner}`);
        }
      } else if (stateTy === "processing") {
        statusCallback("Processing");
        console.log("file PROCESSING");
      } else if (stateTy === "cancelled") {
        statusCallback("Cancelled");
        console.log("file cancelled");
        return reject("operation was cancelled");
      } else if (stateTy === "uploading") {
        statusCallback("Uploading");
        console.log("file UPLOADING");
      } else if (stateTy === "noJobs") {
        statusCallback("No jobs");
        console.log("no file queued");
      } else if (stateTy === "unknown") {
        statusCallback("Unknown");
        console.log("unknown file state");
      } else {
        statusCallback("Communication error");
        console.log(`unknown state message: ${state}`);
      }

      gotStatus = true;
    }).catch(reject)
  };

  if (connection === null) {
    console.error("connection closed");
    statusCallback("Server hung up");
    return reject("connection was closed");
  }

  connection.onMessage.addListener(listen);

  connection.send(NewRequest.transcode(targetURL));

  let statusFn = () => {
    if (connection === null || done) {
      return;
    }

    if (gotStatus) {
      connection.send(NewRequest.status());
    } else {
      console.log("have not received status, debouncing");
    }

    setTimeout(statusFn, 1000);
  };

  setTimeout(statusFn, 1000);

  console.log(`started download for URL: ${targetURL}`);
});

export type ScrapeURLOpts = {
  statusCallback: (status: string) => void;
  resolveOnFirst: boolean;
};


export const scrapeRedditURLs = (url: string, opts: ScrapeURLOpts): Promise<string[]> => (
  new Promise((resolve, reject) => (
    getVRedditFromUser(url, opts.statusCallback).then((url) => {
      let id = vredditID(url);

      if (id === undefined) {
        throw new Error('could not get id from vreddit url');
      }

      const qualities = [
        '1080',
        '720',
        '480',
        '360',
        '96',
      ];
      const mp4Urls = qualities.map((quality) => ({ name: `${quality} mp4`, url: `https://v.redd.it/${id}/DASH_${quality}.mp4` }));
      const urls = [
        ...mp4Urls,
        ...qualities.map((quality) => ({ name: `${quality} legacy`, url: `https://v.redd.it/${id}/DASH_${quality}`})),
      ];

      // dear lord, here we go. the stub video element we'll listen for events on to tell if the video is real.
      const vid = document.createElement('video');
      loadVideos(urls, [], vid, resolve, opts);
    })
    .catch((err) => reject(err))
  ))
);

type URL = {
  url: string;
  name: string;
};

// Recursively calls itself based on events from the past vid's loading. If it loads, we know
// it's a good url, if it fails, we don't.
//
// If resolveOnFirst is true, we'll resolve on the first success with one URL. Otherwise,
// we accumate URLs by walking all of urls before resolving.
const loadVideos = (urls: URL[], valid: string[], vid: HTMLVideoElement, resolve: (_: string[]) => void, opts: ScrapeURLOpts) => {
  const [{ url, name }, ...rem] = urls;
  const once = { once: '' };
  opts.statusCallback(`Checking ${name}`);

  const meta = (_e: Event) => {
    remove();
    clearVideo();

    const newURLs = [...valid, url];

    if (rem.length === 0 || opts.resolveOnFirst) {
      return resolve(newURLs);
    }

    setTimeout(() => {
      loadVideos(rem, newURLs, vid, resolve, opts);
    }, 500);
  };

  const fail = (e: Event) => {
    remove();
    once.once = e.type;
    clearVideo();

    if (rem.length === 0) {
      return resolve(valid);
    }

    setTimeout(() => {
      // debounce since abort could fire with error.
      if (once.once !== e.type) {
        return;
      }

      loadVideos(rem, valid, vid, resolve, opts);
    }, 500);
  };

  const types: { [s: string]: (e: Event) => void } = {
    'abort': fail,
    'error': fail,
    'loadedmetadata': meta,
  };

  const remove = () => (
    Object.keys(types).map((t) => (
      vid.removeEventListener(t, types[t])
    ))
  );

  const clearVideo = () => {
    // TypesScript doesn't allow src to be empty, even though
    // it's perfectly fine and stops the video from loading.
    (vid as any).src = undefined;
  };

  Object.keys(types).map((t) => (
    vid.addEventListener(t, types[t])
  ));

  vid.src = url;
};

type AlwaysProtoResp = Exclude<proto.TranscodeResp['resp'], undefined>;

const parseResponse = (data: Body): Promise<AlwaysProtoResp> => new Promise((resolve, reject) => (
  data.arrayBuffer().then((data: ArrayBuffer) => {
    const { resp } = proto.TranscodeResp.decode(new Uint8Array(data));
    console.log("message received from server");
    console.dir(resp);

    if (resp === undefined) {
      throw Error("server returned an incomplete response");
    } else if (resp.$case === "error") {
      throw Error("server errored: " + resp.error);
    }

    return resolve(resp);
  }).catch(reject)
));


class VRWebSocket {
  private inner: WebSocketAsPromised;
  constructor(addr: string) {
    this.inner = new WebSocketAsPromised(addr);

    this.onClose = this.inner.onClose;
    this.onClose.addListener((v) => {
      console.error(`websocket closed`);
      console.dir(v);
    });
    this.onError = this.inner.onError;
    this.onError.addListener((v) => {
      console.error(`websocket errored`);
      console.dir(v);
    });
    this.onResponse = this.inner.onResponse;
    this.onMessage = this.inner.onMessage;
  }

  public onClose: Channel;
  public onError: Channel;
  public onResponse: Channel;
  public onMessage: Channel;

  public get isOpened(): boolean {
    return this.inner.isOpened;
  }

  public get isOpening(): boolean {
    return this.inner.isOpening;
  }

  public open(): Promise<Event> {
    return this.inner.open()
  }

  public close(_code?: string, _reason?: string): Promise<Event> {
    console.log("closing connection");
    return this.inner.close()
  }

  public send(data: proto.TranscodeReq) {
    console.log("sending data", data);
    this.inner.send(proto.TranscodeReq.encode(data).finish())
  }

  public async sendRequest(data: proto.TranscodeReq): Promise<AlwaysProtoResp> {
    // this.inner.sendRequest expects JSON RPCs. It ties together the response to the
    // request with a injected requestID which we'll eagerly simulate with our non-parallel
    // stream.

    // XXX: this will break when the server begins to send responses that aren't 1:1 with requests.
    const ret = new Promise<AlwaysProtoResp>((resolve, reject) => {
      this.onMessage.addOnceListener((data) => {
        console.log("presumptive response to request", data);
        console.dir(data);
        parseResponse(data).then(resolve, reject)
      });
    });

    this.send(data);

    return ret;
  }
}

const NewRequest = {
  transcode: (url: string): proto.TranscodeReq => ({ req: { $case: 'transcode', transcode: { url } } }),
  handshake: (authz?: string): proto.TranscodeReq => ({ req: { $case: 'handshake', handshake: {
    token: authz === undefined ? undefined : ({ token: { $case: 'v1', v1: Buffer.from(authz, 'base64') }})
  }}}),
  status: (): proto.TranscodeReq => ({ req: { $case: 'status', status: Empty } }),
};
