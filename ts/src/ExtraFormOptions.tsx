import React from 'react';
import { Field } from 'react-final-form';

import Form from 'react-bootstrap/Form';
import Col from 'react-bootstrap/Col';
import { Cookies } from 'react-cookie';

import FormCheck from './FormCheck';
import FinalFormControl from './FinalFormControl';
import * as util from './util';

export const YoutubeDLOptions = () => (
  <>
    <Form.Group as={Col} md="12" lg="8">
      <Form.Label>Authorization Key</Form.Label>
      <Field
        component={FinalFormControl}
        label="Authorization Key"
        name="authz"
        type="password"
        required={true}
        placeholder="Special key provided by the server owner."
      />
    </Form.Group>
    <Form.Group as={Col} md="12" lg="8">
      <Form.Label>Server</Form.Label>
      <Field
        component={FinalFormControl}
        label="Server"
        name="server"
        type="text"
        required={true}
        placeholder={"api.example.com"}
        initialValue={util.ifLocal("localhost:8080")}
      />
    </Form.Group>
  </>
);

export const ScrapeOptions = () => (
  <>
    <Form.Group as={Col} md="12" lg="8">
      <Field
        name="resolveOnFirst"
        label="Return only the highest quality video found."
        component={FormCheck}
      />
    </Form.Group>
  </>
);
