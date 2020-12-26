import React, { useState } from 'react';

import Container from 'react-bootstrap/Container';

import './App.scss';

import Form from './Form';
import URLList from './URLList';

export default () => {
  const [urls, setURLs] = useState<string[] | null>(null);

  return (
    <Container className="app">
      <h2 className="app-header pt-md-5 mb-3">
        v.redd.it converter
      </h2>
      <Form
        setURLs={setURLs}
      />
      <div className="mt-5">
        {
          urls !== null && (
            <URLList urls={urls} video />
          )
        }
      </div>
    </Container>
  );
};
