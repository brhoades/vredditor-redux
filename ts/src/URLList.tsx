import React from 'react';

import Alert from 'react-bootstrap/Alert';
import Col from 'react-bootstrap/Col';
import Form from 'react-bootstrap/Form';

import SelectAndCopyField from './SelectAndCopyField';

const VideoPreview = ({ url }: { url: string }) => (
  <video
    src={url}
    width="240"
    height="auto"
  />
);

const quality = (url: string): string => {
  const res = /DASH_([0-9]+)\.?([a-z0-9]{1,5})?/.exec(url);

  if (res === null) {
    return '';
  }

  if (res?.length > 3) {
    return `${res[1]}p ${res[2]} video`;
  } else if (res?.length > 1) {
    return `${res[1]}p video`;
  } else {
    return '';
  }
};

export default ({ urls, video }: { urls: string[], video?: boolean }) => {
  if (urls.length === 0) {
    return (
      <div className="mt-3">
        <Alert variant="danger">
          Couldn't get a video from the link provided. If it's definitely a v.redd.it video,
          please send <a href="mailto:me@brod.es">me@brod.es</a> an email with the link.
        </Alert>
      </div>
    );
  }

  return (
    <>
      { video && <VideoPreview url={urls[0]} /> }
      <div className="mt-3" />
      {
        urls.map((url: string) => (
          <Col md="8">
            <Form.Group>
              <SelectAndCopyField
                value={url}
                displayValue
              />
              <Form.Label>
                {
                  quality(url)
                    ? (
                      <>
                        {quality(url)}
                        &nbsp;
                        (<a href={url} target="_blank">preview</a>)
                      </>
                    )
                    : (
                      <a href={url} target="_blank">preview</a>
                    )
                }
              </Form.Label>
            </Form.Group>
          </Col>
        ))
      }
    </>
  );
};
